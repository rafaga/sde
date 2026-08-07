use kdtree::KdTree;
use std::collections::HashMap;
use std::ops::{Add, Div, DivAssign, Mul, MulAssign, Sub};

/// Axis choice for a 3D-to-2D projection -- shared between
/// [`MapPoint::to_2d`] (explicit, caller-chosen axis) and
/// `builder::parser::isometric_projection_2d` (the isometric map
/// projection). Lives here, not in `builder`, since `MapPoint` needs it
/// on the read side too, and `objects.rs` isn't gated by the `builder`
/// feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProjectedAxis {
    X,
    /// Default -- matches `SdeConfig.projected_axis = 1` in Python.
    #[default]
    Y,
    Z,
}

/// A point in EVE's universe: real 3D SDE coordinates (`centerX/Y/Z`,
/// meter-scale, up to ~10^17 in magnitude) and 2D map-query results
/// (`get_systempoints`/`get_abstract_systems`, KdTree-indexed) used to
/// be two separate types (`SdePoint`, 3D `i64`, and `MapPoint`, 2D
/// `f32`). They're merged here into one.
///
/// # Why `f64`, not `i64` or `f32`
///
/// `kdtree`'s `KdTree<A, T, U>` requires `A: num_traits::Float` --
/// verified against a real compiler that `i64` does not qualify (only
/// `f32`/`f64` do), so the old `i64`-based `SdePoint` couldn't stay as
/// the KdTree's coordinate type as-is. Between `f32` and `f64`: real
/// EVE coordinates reach ~10^17 in magnitude; `f32`'s ~7 decimal digits
/// of precision can't represent single-meter resolution at that scale
/// (the gap between representable values is in the *billions* of
/// meters), while `f64`'s ~16 digits keeps sub-meter precision even at
/// the largest distances in the game. `f64` exactly represents any
/// `i64` only up to ±2^53 (~9x10^15) -- above that, the `i64 -> f64`
/// conversion in `From<[i64; 3]>` below is no longer bit-perfect, though
/// still far more precise than `f32` would have been.
///
/// # Breaking change from the old `i64`-based `SdePoint`
///
/// This is an intentional breaking change for external consumers that
/// relied on `SdePoint`'s `i64` arithmetic semantics (integer
/// division/truncation; the old `TryInto<[f32;2]>`'s exact-zero-component
/// pivot logic). Confirmed with a real external consumer (`telescope`,
/// `dev` branch): its "center on target" feature calls
/// `get_system_coords(...).try_into().unwrap()` on real, essentially-never-zero
/// EVE coordinates -- reproduced against a real compiler that this
/// panics in practice, every time, regardless of this merge. The old
/// pivot-based `TryInto<[f32;2]>` is replaced here by [`MapPoint::to_2d`],
/// which takes an explicit [`ProjectedAxis`] instead of guessing one
/// from a component that's essentially never exactly zero for real data.
/// `telescope` needs updating either way to fix this bug; this merge
/// doesn't add new work there, just changes what the fix looks like.
///
/// The scalar arithmetic (`Add`/`Sub`/`MulAssign`/`DivAssign`) kept below
/// covers only what's confirmed in actual use, internally
/// (`SdeManager::get_system_coords`'s `factor`/`invert_coordinates`
/// scaling, which needs `i64`-typed scalars) and externally (`telescope`
/// reads `.x`/`.y`/`.z`-equivalent components directly, via
/// [`MapPoint::x`]/[`MapPoint::y`]/[`MapPoint::z`] after this merge, not
/// through the arithmetic operators). The old `SdePoint` additionally
/// implemented `Mul`/`Div`/`MulAssign`/`DivAssign` for `isize`/`u64`/`i32`/`f32`
/// scalars and a value-returning `Mul<isize>`/`Div<isize>` -- none of
/// which had any confirmed caller, internal or external, so they aren't
/// carried over. If some caller does need one of these, they're
/// straightforward to add back.
#[derive(Debug, Clone, PartialEq)]
pub struct MapPoint {
    pub coords: [f64; 3],
    /// `None` for a bare coordinate (bounding-box corners, real/projected
    /// system positions inside `SolarSystem`/`Constellation`/`Region`) --
    /// `Some` for a query result with an actual entity behind it
    /// (`get_systempoints`/`get_abstract_systems`).
    pub id: Option<usize>,
    pub name: Option<String>,
    /// `(solarSystemId, solarSystemId)` pairs for this point's
    /// connections to others -- each entry corresponds to the `id` of a
    /// [`MapSegment`] returned by [`crate::SdeManager::get_connections`]
    /// (or `get_abstract_connections`). Empty for a bare coordinate.
    pub connections: Vec<(usize, usize)>,
}

impl MapPoint {
    /// Creates a bare coordinate (`id`/`name` `None`, no connections) --
    /// equivalent to the old `SdePoint::new(x, y, z)`.
    pub fn new(x: f64, y: f64, z: f64) -> Self {
        Self {
            coords: [x, y, z],
            id: None,
            name: None,
            connections: Vec::new(),
        }
    }

    pub fn x(&self) -> f64 {
        self.coords[0]
    }

    pub fn y(&self) -> f64 {
        self.coords[1]
    }

    pub fn z(&self) -> f64 {
        self.coords[2]
    }

    /// Explicit 2D projection, dropping `axis`'s component -- replaces
    /// the old pivot-based `TryInto<[f32;2]>` (see the struct's
    /// docstring for why that one is gone, not just moved).
    pub fn to_2d(&self, axis: ProjectedAxis) -> [f32; 2] {
        match axis {
            ProjectedAxis::X => [self.coords[1] as f32, self.coords[2] as f32],
            ProjectedAxis::Y => [self.coords[0] as f32, self.coords[2] as f32],
            ProjectedAxis::Z => [self.coords[0] as f32, self.coords[1] as f32],
        }
    }
}

impl Default for MapPoint {
    fn default() -> Self {
        Self::new(0.0, 0.0, 0.0)
    }
}

impl From<[i64; 3]> for MapPoint {
    fn from(value: [i64; 3]) -> Self {
        Self::new(value[0] as f64, value[1] as f64, value[2] as f64)
    }
}

impl From<[f32; 3]> for MapPoint {
    fn from(value: [f32; 3]) -> Self {
        Self::new(value[0] as f64, value[1] as f64, value[2] as f64)
    }
}

impl From<[f64; 3]> for MapPoint {
    fn from(value: [f64; 3]) -> Self {
        Self::new(value[0], value[1], value[2])
    }
}

impl From<MapPoint> for [i64; 3] {
    fn from(val: MapPoint) -> Self {
        [
            val.coords[0].round() as i64,
            val.coords[1].round() as i64,
            val.coords[2].round() as i64,
        ]
    }
}

impl From<MapPoint> for [f64; 3] {
    fn from(val: MapPoint) -> Self {
        val.coords
    }
}

impl DivAssign<i64> for MapPoint {
    fn div_assign(&mut self, rhs: i64) {
        self.coords[0] /= rhs as f64;
        self.coords[1] /= rhs as f64;
        self.coords[2] /= rhs as f64;
    }
}

impl MulAssign<i64> for MapPoint {
    fn mul_assign(&mut self, rhs: i64) {
        self.coords[0] *= rhs as f64;
        self.coords[1] *= rhs as f64;
        self.coords[2] *= rhs as f64;
    }
}

impl Mul<f64> for MapPoint {
    type Output = Self;
    fn mul(mut self, rhs: f64) -> Self::Output {
        self.coords[0] *= rhs;
        self.coords[1] *= rhs;
        self.coords[2] *= rhs;
        self
    }
}

impl Div<f64> for MapPoint {
    type Output = Self;
    fn div(mut self, rhs: f64) -> Self::Output {
        self.coords[0] /= rhs;
        self.coords[1] /= rhs;
        self.coords[2] /= rhs;
        self
    }
}

impl Add<MapPoint> for MapPoint {
    type Output = MapPoint;
    fn add(self, rhs: MapPoint) -> Self::Output {
        MapPoint::new(
            self.coords[0] + rhs.coords[0],
            self.coords[1] + rhs.coords[1],
            self.coords[2] + rhs.coords[2],
        )
    }
}

impl Sub<MapPoint> for MapPoint {
    type Output = MapPoint;
    fn sub(self, rhs: MapPoint) -> Self::Output {
        MapPoint::new(
            self.coords[0] - rhs.coords[0],
            self.coords[1] - rhs.coords[1],
            self.coords[2] - rhs.coords[2],
        )
    }
}

impl Add<&MapPoint> for MapPoint {
    type Output = MapPoint;
    fn add(self, rhs: &MapPoint) -> Self::Output {
        MapPoint::new(
            self.coords[0] + rhs.coords[0],
            self.coords[1] + rhs.coords[1],
            self.coords[2] + rhs.coords[2],
        )
    }
}

impl Sub<&MapPoint> for MapPoint {
    type Output = MapPoint;
    fn sub(self, rhs: &MapPoint) -> Self::Output {
        MapPoint::new(
            self.coords[0] - rhs.coords[0],
            self.coords[1] - rhs.coords[1],
            self.coords[2] - rhs.coords[2],
        )
    }
}

/// Line segment between two solar systems (a stargate connection, or an
/// edge on the abstract map). A type owned by `sde`, replaces the
/// `MapSegment` that used to come from `egui-map`.
///
/// `id` is the pair of system ids it connects -- no longer an arbitrary
/// `Rc<str>` like in the old `egui-map` integration. This `(usize,
/// usize)` is exactly the shape `egui_map::MapSegment.id` would need
/// after the targeted change already designed for that crate (`Rc<str>`
/// -> tuple) -- the only piece of `egui-map` that needs touching to
/// interoperate, without redesigning anything else there. `point1`/
/// `point2` already match `RawPoint.components` 1:1, requiring no
/// change at all.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MapSegment {
    pub id: (usize, usize),
    pub point1: [f32; 2],
    pub point2: [f32; 2],
}

/// Converts a `KdTree<f64, MapPoint, [f64; 3]>` into a list of
/// references to its points, **without cloning** -- for anyone who
/// prefers to iterate/consume it as a plain list instead of using
/// `kdtree`'s native spatial queries (`nearest`, `within`,
/// `iter_nearest`, etc., all available directly on the tree returned by
/// [`crate::SdeManager::get_systempoints`]/`get_abstract_systems`).
///
/// Internally uses `bounding_box` with bounds at `f64::MIN`/`f64::MAX`
/// (the full representable finite range) to fetch the tree's entire
/// content in one go -- this is the exact usage `kdtree`'s own
/// documentation recommends for "range queries where you only need the
/// references, without ordering guarantees". Important:
/// `f64::INFINITY`/`f64::NEG_INFINITY` do NOT work here -- `kdtree`
/// explicitly rejects them as non-finite bounds
/// (`ErrorKind::NonFiniteCoordinate`); verified against the crate's
/// actual source code, not just its documentation.
///
/// Can't fail in practice (3 fixed dimensions, always-finite bounds),
/// so `expect` is used instead of propagating a `Result` that would
/// never be `Err` with this usage -- if it ever were, that's a sign of
/// a real bug that should abort loudly, not get silently swallowed into
/// an empty list.
pub fn map_points_to_vec(tree: &KdTree<f64, MapPoint, [f64; 3]>) -> Vec<&MapPoint> {
    tree.bounding_box(&[f64::MIN, f64::MIN, f64::MIN], &[f64::MAX, f64::MAX, f64::MAX])
        .expect("bounding_box with f64::MIN/f64::MAX bounds should never fail (3 fixed dimensions, always-finite bounds)")
}

/// Note: no longer derives `Hash`/`Eq` (only `PartialEq`) since `min`/`max`
/// became `MapPoint`, which contains `[f64; 3]` -- `f64` doesn't
/// implement `Eq`/`Hash` (NaN isn't equal to itself). Nothing in this
/// crate used `EveRegionArea` as a `HashMap`/`HashSet` key or otherwise
/// relied on those two traits (confirmed: it's only ever returned inside
/// a `Vec`).
#[derive(PartialEq, Clone, Debug)]
pub struct EveRegionArea {
    pub region_id: i64,
    pub name: String,
    pub min: MapPoint,
    pub max: MapPoint,
}

impl Default for EveRegionArea {
    fn default() -> Self {
        Self::new()
    }
}

impl EveRegionArea {
    pub fn new() -> Self {
        EveRegionArea {
            region_id: 0,
            name: String::new(),
            min: MapPoint::default(),
            max: MapPoint::default(),
        }
    }
}

#[derive(Hash, PartialEq, Eq, Clone, Debug)]
pub struct Moon {
    /// Moon Identifier
    pub id: u32,
    /// Moon's Planet identifier
    pub planet: u32,
    /// The cardinal number of this moon in the planet
    pub index: u8,
    /// Moon's Solar System Identifier
    pub solar_system: u32,
}

impl Moon {
    /// Creates a new Moon Strcut. ALl the values are initialized. Needs to be filled
    pub fn new() -> Self {
        Moon {
            id: 0,
            planet: 0,
            index: 0,
            solar_system: 0,
        }
    }
}

impl Default for Moon {
    fn default() -> Self {
        Self::new()
    }
}

/// Abstraction for a Planet. It store data relevant to this entity
#[derive(Hash, PartialEq, Eq, Clone, Debug)]
pub struct Planet {
    /// Planet identifier
    pub id: u32,
    /// Planet's Solar System Idetifier
    pub solar_system: u32,
    /// The cardinal number of this planet in the solar system.
    pub index: u8,
}

impl Planet {
    /// Creates a new Planet Strcut. ALl the values are initialized. Needs to be filled
    pub fn new() -> Self {
        Planet {
            id: 0,
            solar_system: 0,
            index: 0,
        }
    }
}

impl Default for Planet {
    fn default() -> Self {
        Self::new()
    }
}

/// Abstraction for a Solar System. It store data relevant to this entity
///
/// Note: no longer derives `Hash`/`Eq` (only `PartialEq`) -- same reason
/// as [`EveRegionArea`]: `real_coords`/`projected_coords` became
/// `MapPoint`, which contains `[f64; 3]`. Confirmed safe: `SolarSystem`
/// is only ever a `HashMap` *value* (`HashMap<u32, SolarSystem>`), never
/// a key.
#[derive(PartialEq, Clone, Debug)]
pub struct SolarSystem {
    /// Solar System identifier
    pub id: u32,
    /// Solar System name
    pub name: String,
    /// Region identifier
    pub region: u32,
    /// Constellation identifier
    pub constellation: u32,
    /// Planet vector with Identifer numbers in their respective cardinal order
    pub planets: Vec<u32>,
    /// Vector with Solar system identifiers where this Solar system has connections via Stargates
    pub connections: Vec<u32>,
    /// Solar System 3D Coordinates
    pub real_coords: MapPoint,
    /// Solar System 2D Coordinates with the propourse of representing the system in abstraction map.
    pub projected_coords: MapPoint,
    /// The factor that we need to adjust the coordinates
    pub factor: i64,
}

impl SolarSystem {
    /// Creates a new Solar System Strcut. ALl the values are initialized. Needs to be filled
    pub fn new(factor: i64) -> Self {
        SolarSystem {
            id: 0,
            name: String::new(),
            region: 0,
            constellation: 0,
            planets: Vec::new(),
            connections: Vec::new(),
            real_coords: MapPoint::default(),
            projected_coords: MapPoint::default(),
            factor,
        }
    }
}

impl Default for SolarSystem {
    fn default() -> Self {
        Self::new(1)
    }
}

/// Abstraction for a Constellation. It store data relevant to this entity
///
/// Note: no longer derives `Hash`/`Eq`, same reason and same
/// confirmation (`HashMap` value, never a key) as [`SolarSystem`].
#[derive(PartialEq, Clone, Debug)]
pub struct Constellation {
    /// Constellation Identifier
    pub id: u32,
    /// Constellation Name
    pub name: String,
    /// Region Identifier
    pub region: u32,
    /// Solar System vector with Identifer numbers included in the constellation
    pub solar_systems: Vec<u32>,
    /// Solar System 2D Coordinates with the propourse of representing the system in abstraction map.
    pub projected_coords: MapPoint,
}

impl Constellation {
    /// Creates a new Constellation Strcut. ALl the values are initialized. Needs to be filled
    pub fn new() -> Self {
        Constellation {
            id: 0,
            name: String::new(),
            region: 0,
            solar_systems: Vec::new(),
            projected_coords: MapPoint::default(),
        }
    }
}

impl Default for Constellation {
    fn default() -> Self {
        Self::new()
    }
}

/// Abstraction for a Region. It store data relevant to this entity
///
/// Note: no longer derives `Hash`/`Eq`, same reason and same
/// confirmation (`HashMap` value, never a key) as [`SolarSystem`].
#[derive(PartialEq, Clone, Debug)]
pub struct Region {
    /// Region Identifier
    pub id: u32,
    /// Region Name
    pub name: String,
    /// Vector with Region's Constellationm Identifiers
    pub constellations: Vec<u32>,
    /// Region 2D Coordinates with the propourse of representing the system in abstraction map.
    pub projected_coords: MapPoint,
}

impl Region {
    /// Creates a new Region Strcut. ALl the values are initialized. Needs to be filled
    pub fn new() -> Self {
        Region {
            id: 0,
            name: String::new(),
            constellations: Vec::new(),
            projected_coords: MapPoint::default(),
        }
    }
}

impl Default for Region {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone)]
/// Struct that contains everything in EVE Onoline Universe
///
/// - Regions
/// - Constellations
/// - SolarSystems
/// - Planets
/// - Moons
/// - and the object dictionaries
pub struct Universe {
    /// Region objects you can access the data with their Identfiers
    pub regions: HashMap<u32, Region>,
    /// Constellation objects you can access the data with their Identfiers
    pub constellations: HashMap<u32, Constellation>,
    /// Solarsystem objects you can access the data with their Identfiers
    pub solar_systems: HashMap<u32, SolarSystem>,
    /// Planet objects you can access the data with their Identfiers
    pub planets: HashMap<u32, Planet>,
    /// Moon objects you can access the data with their Identfiers
    pub moons: HashMap<u32, Moon>,
    /// Factor used to correct coordinates
    pub factor: i64,
}

impl Universe {
    /// Creates a new Universe Strcut. ALl the values are initialized. Needs to be filled
    pub fn new(factor: i64) -> Universe {
        Universe {
            regions: HashMap::new(),
            constellations: HashMap::new(),
            solar_systems: HashMap::new(),
            planets: HashMap::new(),
            moons: HashMap::new(),
            factor,
        }
    }
}

impl Default for Universe {
    fn default() -> Self {
        Self::new(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------------------------------------------------------------
    // MapPoint
    // ---------------------------------------------------------------------

    #[test]
    fn mappoint_new_sets_coordinates() {
        let point = MapPoint::new(10.0, -20.0, 30.0);
        assert_eq!(point.x(), 10.0);
        assert_eq!(point.y(), -20.0);
        assert_eq!(point.z(), 30.0);
        assert_eq!(point.id, None);
        assert_eq!(point.name, None);
        assert!(point.connections.is_empty());
    }

    #[test]
    fn mappoint_default_is_origin() {
        let point = MapPoint::default();
        assert_eq!(point.coords, [0.0, 0.0, 0.0]);
        assert_eq!(point, MapPoint::new(0.0, 0.0, 0.0));
    }

    #[test]
    fn mappoint_from_i64_array() {
        let point = MapPoint::from([1i64, 2, 3]);
        assert_eq!(point, MapPoint::new(1.0, 2.0, 3.0));
    }

    #[test]
    fn mappoint_from_f32_array() {
        let point = MapPoint::from([1.4f32, 1.5, -1.5]);
        assert_eq!(point, MapPoint::new(1.4f32 as f64, 1.5, -1.5));
    }

    #[test]
    fn mappoint_from_f64_array() {
        let point = MapPoint::from([1.4, 1.5, -1.5]);
        assert_eq!(point, MapPoint::new(1.4, 1.5, -1.5));
    }

    #[test]
    fn mappoint_into_i64_array_rounds() {
        // f64 -> i64 rounds (half away from zero), unlike the f64 array
        // conversion below, which is lossless.
        let values: [i64; 3] = MapPoint::new(7.4, 7.5, -7.5).into();
        assert_eq!(values, [7, 8, -8]);
    }

    #[test]
    fn mappoint_into_f64_array() {
        let values: [f64; 3] = MapPoint::new(7.0, 8.0, 9.0).into();
        assert_eq!(values, [7.0, 8.0, 9.0]);
    }

    #[test]
    fn mappoint_to_2d_drops_x() {
        let point = MapPoint::new(10.0, 20.0, 30.0);
        assert_eq!(point.to_2d(ProjectedAxis::X), [20.0, 30.0]);
    }

    #[test]
    fn mappoint_to_2d_drops_y() {
        let point = MapPoint::new(10.0, 20.0, 30.0);
        assert_eq!(point.to_2d(ProjectedAxis::Y), [10.0, 30.0]);
    }

    #[test]
    fn mappoint_to_2d_drops_z() {
        let point = MapPoint::new(10.0, 20.0, 30.0);
        assert_eq!(point.to_2d(ProjectedAxis::Z), [10.0, 20.0]);
    }

    #[test]
    fn mappoint_to_2d_never_fails_even_at_the_pivot_values_that_used_to_panic() {
        // The point that reproduced telescope's real panic (see the
        // struct's docstring): no component is exactly zero, so the old
        // pivot-based TryInto<[f32;2]> would return Err on all three
        // branches, and telescope's `.unwrap()` would panic. `to_2d`
        // can't fail -- it doesn't guess, the caller picks the axis.
        let point = MapPoint::new(
            1_003_094_336_444_825.0,
            -2_005_029_375_317_114.0,
            3_001_839_229_715_087.0,
        );
        let _ = point.to_2d(ProjectedAxis::Y); // must not panic
    }

    #[test]
    fn mappoint_add_owned() {
        let sum = MapPoint::new(1.0, 2.0, 3.0) + MapPoint::new(10.0, 20.0, 30.0);
        assert_eq!(sum, MapPoint::new(11.0, 22.0, 33.0));
    }

    #[test]
    fn mappoint_add_reference() {
        let sum = MapPoint::new(1.0, 2.0, 3.0) + &MapPoint::new(-1.0, -2.0, -3.0);
        assert_eq!(sum, MapPoint::new(0.0, 0.0, 0.0));
    }

    #[test]
    fn mappoint_sub_owned() {
        let diff = MapPoint::new(10.0, 20.0, 30.0) - MapPoint::new(1.0, 2.0, 3.0);
        assert_eq!(diff, MapPoint::new(9.0, 18.0, 27.0));
    }

    #[test]
    fn mappoint_sub_reference() {
        let diff = MapPoint::new(10.0, 20.0, 30.0) - &MapPoint::new(10.0, 20.0, 30.0);
        assert_eq!(diff, MapPoint::new(0.0, 0.0, 0.0));
    }

    #[test]
    fn mappoint_mul_assign_i64() {
        // Matches SdeManager::get_system_coords()'s `coord *= self.factor.abs()`.
        let mut point = MapPoint::new(1.0, 2.0, 3.0);
        point *= 3i64;
        assert_eq!(point, MapPoint::new(3.0, 6.0, 9.0));
    }

    #[test]
    fn mappoint_div_assign_i64() {
        // Matches SdeManager::get_system_coords()'s `coord /= self.factor`.
        let mut point = MapPoint::new(24.0, 48.0, 96.0);
        point /= 2i64;
        assert_eq!(point, MapPoint::new(12.0, 24.0, 48.0));
    }

    #[test]
    fn mappoint_mul_f64() {
        let product = MapPoint::new(1.0, -2.0, 3.0) * 2.5;
        assert_eq!(product, MapPoint::new(2.5, -5.0, 7.5));
    }

    #[test]
    fn mappoint_div_f64() {
        let quotient = MapPoint::new(10.0, -20.0, 30.0) / 4.0;
        assert_eq!(quotient, MapPoint::new(2.5, -5.0, 7.5));
    }

    // ---------------------------------------------------------------------
    // EveRegionArea
    // ---------------------------------------------------------------------

    #[test]
    fn everegionarea_new_is_empty() {
        let area = EveRegionArea::new();
        assert_eq!(area.region_id, 0);
        assert_eq!(area.name, String::new());
        assert_eq!(area.min, MapPoint::default());
        assert_eq!(area.max, MapPoint::default());
        assert_eq!(area, EveRegionArea::default());
    }

    // ---------------------------------------------------------------------
    // Moon / Planet
    // ---------------------------------------------------------------------

    #[test]
    fn moon_new_is_zeroed() {
        let moon = Moon::new();
        assert_eq!(moon.id, 0);
        assert_eq!(moon.planet, 0);
        assert_eq!(moon.index, 0);
        assert_eq!(moon.solar_system, 0);
        assert_eq!(moon, Moon::default());
    }

    #[test]
    fn planet_new_is_zeroed() {
        let planet = Planet::new();
        assert_eq!(planet.id, 0);
        assert_eq!(planet.solar_system, 0);
        assert_eq!(planet.index, 0);
        assert_eq!(planet, Planet::default());
    }

    // ---------------------------------------------------------------------
    // SolarSystem
    // ---------------------------------------------------------------------

    #[test]
    fn solarsystem_new_initializes_with_factor() {
        let system = SolarSystem::new(1000);
        assert_eq!(system.id, 0);
        assert_eq!(system.name, String::new());
        assert_eq!(system.region, 0);
        assert_eq!(system.constellation, 0);
        assert!(system.planets.is_empty());
        assert!(system.connections.is_empty());
        assert_eq!(system.real_coords, MapPoint::default());
        assert_eq!(system.projected_coords, MapPoint::default());
        assert_eq!(system.factor, 1000);
    }

    #[test]
    fn solarsystem_default_factor_is_one() {
        assert_eq!(SolarSystem::default().factor, 1);
    }

    // ---------------------------------------------------------------------
    // Constellation / Region / Universe
    // ---------------------------------------------------------------------

    #[test]
    fn constellation_new_is_empty() {
        let constellation = Constellation::new();
        assert_eq!(constellation.id, 0);
        assert_eq!(constellation.name, String::new());
        assert_eq!(constellation.region, 0);
        assert!(constellation.solar_systems.is_empty());
        assert_eq!(constellation.projected_coords, MapPoint::default());
        assert_eq!(constellation, Constellation::default());
    }

    #[test]
    fn region_new_is_empty() {
        let region = Region::new();
        assert_eq!(region.id, 0);
        assert_eq!(region.name, String::new());
        assert!(region.constellations.is_empty());
        assert_eq!(region.projected_coords, MapPoint::default());
        assert_eq!(region, Region::default());
    }

    #[test]
    fn universe_new_initializes_with_factor() {
        let universe = Universe::new(42);
        assert!(universe.regions.is_empty());
        assert!(universe.constellations.is_empty());
        assert!(universe.solar_systems.is_empty());
        assert!(universe.planets.is_empty());
        assert!(universe.moons.is_empty());
        assert_eq!(universe.factor, 42);
    }

    #[test]
    fn universe_default_factor_is_one() {
        assert_eq!(Universe::default().factor, 1);
    }
}
