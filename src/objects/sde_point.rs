use super::ProjectedAxis;
use std::ops::{Add, Div, DivAssign, Mul, MulAssign, Sub};

/// A point in EVE's universe: real 3D SDE coordinates (`centerX/Y/Z`)
/// and 2D map-query results (`get_systems`/`get_abstract_systems`)
/// used to be two separate types (`SdePoint`, 3D `i64`, and `SdePoint`,
/// 2D `f32`). They're merged here into one.
///
/// # Why `f64`, not `i64` or `f32`
///
/// Checked against real `mapRegions.jsonl`/`mapSolarSystems.jsonl`
/// samples (August 2026): real coordinates reach ~1.0x10^19 in
/// magnitude -- `f32`'s ~7 decimal digits of precision can't represent
/// single-meter resolution anywhere near that scale, while `f64`'s ~16
/// digits keeps the error in the tens-to-thousands-of-meters range
/// even at the largest distances found -- negligible at interstellar
/// scale, but worth being precise about (see the next section for why
/// `f64` isn't perfectly exact here either). `i64` doesn't fit either:
/// `centerX`/`centerY`/`centerZ`/`position2DX`/`position2DY` are all
/// `REAL` columns in `schema.sql`, so reading them as `f64` is a direct
/// match with no conversion, while `i64` would need a lossy
/// truncation on every read for data that's genuinely fractional to
/// begin with.
///
/// # `i64 -> f64` isn't bit-perfect above 2^53 -- confirmed to matter for real data
///
/// `f64` exactly represents any `i64` only up to ±2^53 (~9.0x10^15).
/// The same real samples checked above show this isn't a theoretical
/// edge case: **~95% of individual `x`/`y`/`z` components exceed 2^53**
/// (up to ~1115x the threshold, in `mapSolarSystems.jsonl`), so an
/// `i64 -> f64` round-trip on real region/system coordinates routinely
/// isn't bit-perfect.
///
/// This matters less than it sounds, in practice, for two reasons: the
/// resulting error stays in the tens-to-thousands-of-meters range even
/// at the largest magnitudes found (`f64`'s ~16 significant digits, not
/// `f32`'s ~7) -- negligible against distances measured in light-years
/// -- and, as of this merge, **no code path inside this crate actually
/// performs that conversion for real coordinate data anymore**: the two
/// call sites that used to (`SdeManager::get_system_coords`,
/// `SdeManager::get_solarsystem`) were rewritten to read `f64` directly
/// from SQLite and build a `SdePoint` without an `i64` intermediate.
/// `From<[i64; 3]>`/`From<SdePoint> for [i64; 3]` are kept as public API
/// for external consumers that still need them (confirmed in use, see
/// below) -- anyone converting real, large-magnitude coordinates
/// through them should keep this precision note in mind.
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
/// pivot-based `TryInto<[f32;2]>` is replaced here by [`SdePoint::to_2d`],
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
/// [`SdePoint::x`]/[`SdePoint::y`]/[`SdePoint::z`] after this merge, not
/// through the arithmetic operators). The old `SdePoint` additionally
/// implemented `Mul`/`Div`/`MulAssign`/`DivAssign` for `isize`/`u64`/`i32`/`f32`
/// scalars and a value-returning `Mul<isize>`/`Div<isize>` -- none of
/// which had any confirmed caller, internal or external, so they aren't
/// carried over. If some caller does need one of these, they're
/// straightforward to add back.
#[derive(Debug, Clone, PartialEq)]
pub struct SdePoint {
    pub coords: [f64; 3],
    /// `None` for a bare coordinate (bounding-box corners, real/projected
    /// system positions inside `SolarSystem`/`Constellation`/`Region`) --
    /// `Some` for a query result with an actual entity behind it
    /// (`get_systems`/`get_abstract_systems`).
    pub id: Option<usize>,
    pub name: Option<String>,
    /// `(solarSystemId, solarSystemId)` pairs for this point's
    /// connections to others -- each entry corresponds to the `id` of a
    /// [`SdeSegment`](super::SdeSegment) returned by [`crate::SdeManager::get_connections`]
    /// (or `get_abstract_connections`). Empty for a bare coordinate.
    pub connections: Vec<(usize, usize)>,
    /// Hex RGB color of this system's star (`typeStar.color`, e.g.
    /// `"#FFE996"`), shared by every star of the same spectral class --
    /// see [`Star::color`](crate::objects::Star::color). `None` for a
    /// bare coordinate, and for a real system with no `mapStars` row
    /// (same 401-of-8490 gap noted on [`SolarSystem::star`](super::SolarSystem::star)).
    ///
    /// Deliberately kept as the raw hex string rather than a parsed
    /// color type: `sde` has no rendering-library dependency (not even
    /// `egui-map`, see the crate-level notes) and shouldn't gain one
    /// just for this field -- whoever paints the point decides how (or
    /// whether) to parse it.
    pub color: Option<String>,
}

impl SdePoint {
    /// Creates a bare coordinate (`id`/`name` `None`, no connections) --
    /// equivalent to the old `SdePoint::new(x, y, z)`.
    pub fn new(x: f64, y: f64, z: f64) -> Self {
        Self {
            coords: [x, y, z],
            id: None,
            name: None,
            connections: Vec::new(),
            color: None,
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

impl Default for SdePoint {
    fn default() -> Self {
        Self::new(0.0, 0.0, 0.0)
    }
}

impl From<[i64; 3]> for SdePoint {
    fn from(value: [i64; 3]) -> Self {
        Self::new(value[0] as f64, value[1] as f64, value[2] as f64)
    }
}

impl From<[f32; 3]> for SdePoint {
    fn from(value: [f32; 3]) -> Self {
        Self::new(value[0] as f64, value[1] as f64, value[2] as f64)
    }
}

impl From<[f64; 3]> for SdePoint {
    fn from(value: [f64; 3]) -> Self {
        Self::new(value[0], value[1], value[2])
    }
}

impl From<SdePoint> for [i64; 3] {
    fn from(val: SdePoint) -> Self {
        [
            val.coords[0].round() as i64,
            val.coords[1].round() as i64,
            val.coords[2].round() as i64,
        ]
    }
}

impl From<SdePoint> for [f64; 3] {
    fn from(val: SdePoint) -> Self {
        val.coords
    }
}

impl DivAssign<f64> for SdePoint {
    fn div_assign(&mut self, rhs: f64) {
        self.coords[0] /= rhs;
        self.coords[1] /= rhs;
        self.coords[2] /= rhs;
    }
}

impl MulAssign<f64> for SdePoint {
    fn mul_assign(&mut self, rhs: f64) {
        self.coords[0] *= rhs;
        self.coords[1] *= rhs;
        self.coords[2] *= rhs;
    }
}

impl Mul<f64> for SdePoint {
    type Output = Self;
    fn mul(mut self, rhs: f64) -> Self::Output {
        self.coords[0] *= rhs;
        self.coords[1] *= rhs;
        self.coords[2] *= rhs;
        self
    }
}

impl Div<f64> for SdePoint {
    type Output = Self;
    fn div(mut self, rhs: f64) -> Self::Output {
        self.coords[0] /= rhs;
        self.coords[1] /= rhs;
        self.coords[2] /= rhs;
        self
    }
}

impl Add<SdePoint> for SdePoint {
    type Output = SdePoint;
    fn add(self, rhs: SdePoint) -> Self::Output {
        SdePoint::new(
            self.coords[0] + rhs.coords[0],
            self.coords[1] + rhs.coords[1],
            self.coords[2] + rhs.coords[2],
        )
    }
}

impl Sub<SdePoint> for SdePoint {
    type Output = SdePoint;
    fn sub(self, rhs: SdePoint) -> Self::Output {
        SdePoint::new(
            self.coords[0] - rhs.coords[0],
            self.coords[1] - rhs.coords[1],
            self.coords[2] - rhs.coords[2],
        )
    }
}

impl Add<&SdePoint> for SdePoint {
    type Output = SdePoint;
    fn add(self, rhs: &SdePoint) -> Self::Output {
        SdePoint::new(
            self.coords[0] + rhs.coords[0],
            self.coords[1] + rhs.coords[1],
            self.coords[2] + rhs.coords[2],
        )
    }
}

impl Sub<&SdePoint> for SdePoint {
    type Output = SdePoint;
    fn sub(self, rhs: &SdePoint) -> Self::Output {
        SdePoint::new(
            self.coords[0] - rhs.coords[0],
            self.coords[1] - rhs.coords[1],
            self.coords[2] - rhs.coords[2],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdepoint_new_sets_coordinates() {
        let point = SdePoint::new(10.0, -20.0, 30.0);
        assert_eq!(point.x(), 10.0);
        assert_eq!(point.y(), -20.0);
        assert_eq!(point.z(), 30.0);
        assert_eq!(point.id, None);
        assert_eq!(point.name, None);
        assert!(point.connections.is_empty());
    }

    #[test]
    fn sdepoint_default_is_origin() {
        let point = SdePoint::default();
        assert_eq!(point.coords, [0.0, 0.0, 0.0]);
        assert_eq!(point, SdePoint::new(0.0, 0.0, 0.0));
    }

    #[test]
    fn sdepoint_from_i64_array() {
        let point = SdePoint::from([1i64, 2, 3]);
        assert_eq!(point, SdePoint::new(1.0, 2.0, 3.0));
    }

    #[test]
    fn sdepoint_from_f32_array() {
        let point = SdePoint::from([1.4f32, 1.5, -1.5]);
        assert_eq!(point, SdePoint::new(1.4f32 as f64, 1.5, -1.5));
    }

    #[test]
    fn sdepoint_from_f64_array() {
        let point = SdePoint::from([1.4, 1.5, -1.5]);
        assert_eq!(point, SdePoint::new(1.4, 1.5, -1.5));
    }

    #[test]
    fn sdepoint_into_i64_array_rounds() {
        // f64 -> i64 rounds (half away from zero), unlike the f64 array
        // conversion below, which is lossless.
        let values: [i64; 3] = SdePoint::new(7.4, 7.5, -7.5).into();
        assert_eq!(values, [7, 8, -8]);
    }

    #[test]
    fn sdepoint_into_f64_array() {
        let values: [f64; 3] = SdePoint::new(7.0, 8.0, 9.0).into();
        assert_eq!(values, [7.0, 8.0, 9.0]);
    }

    #[test]
    fn sdepoint_to_2d_drops_x() {
        let point = SdePoint::new(10.0, 20.0, 30.0);
        assert_eq!(point.to_2d(ProjectedAxis::X), [20.0, 30.0]);
    }

    #[test]
    fn sdepoint_to_2d_drops_y() {
        let point = SdePoint::new(10.0, 20.0, 30.0);
        assert_eq!(point.to_2d(ProjectedAxis::Y), [10.0, 30.0]);
    }

    #[test]
    fn sdepoint_to_2d_drops_z() {
        let point = SdePoint::new(10.0, 20.0, 30.0);
        assert_eq!(point.to_2d(ProjectedAxis::Z), [10.0, 20.0]);
    }

    #[test]
    fn sdepoint_to_2d_never_fails_even_at_the_pivot_values_that_used_to_panic() {
        // The point that reproduced telescope's real panic (see the
        // struct's docstring): no component is exactly zero, so the old
        // pivot-based TryInto<[f32;2]> would return Err on all three
        // branches, and telescope's `.unwrap()` would panic. `to_2d`
        // can't fail -- it doesn't guess, the caller picks the axis.
        let point = SdePoint::new(
            1_003_094_336_444_825.0,
            -2_005_029_375_317_114.0,
            3_001_839_229_715_087.0,
        );
        let _ = point.to_2d(ProjectedAxis::Y); // must not panic
    }

    #[test]
    fn sdepoint_add_owned() {
        let sum = SdePoint::new(1.0, 2.0, 3.0) + SdePoint::new(10.0, 20.0, 30.0);
        assert_eq!(sum, SdePoint::new(11.0, 22.0, 33.0));
    }

    #[test]
    fn sdepoint_add_reference() {
        let sum = SdePoint::new(1.0, 2.0, 3.0) + &SdePoint::new(-1.0, -2.0, -3.0);
        assert_eq!(sum, SdePoint::new(0.0, 0.0, 0.0));
    }

    #[test]
    fn sdepoint_sub_owned() {
        let diff = SdePoint::new(10.0, 20.0, 30.0) - SdePoint::new(1.0, 2.0, 3.0);
        assert_eq!(diff, SdePoint::new(9.0, 18.0, 27.0));
    }

    #[test]
    fn sdepoint_sub_reference() {
        let diff = SdePoint::new(10.0, 20.0, 30.0) - &SdePoint::new(10.0, 20.0, 30.0);
        assert_eq!(diff, SdePoint::new(0.0, 0.0, 0.0));
    }

    #[test]
    fn sdepoint_mul_assign_i64() {
        // Matches SdeManager::get_system_coords()'s `coord *= self.factor.abs()`.
        let mut point = SdePoint::new(1.0, 2.0, 3.0);
        point *= 3.0;
        assert_eq!(point, SdePoint::new(3.0, 6.0, 9.0));
    }

    #[test]
    fn sdepoint_div_assign_i64() {
        // Matches SdeManager::get_system_coords()'s `coord /= self.factor`.
        let mut point = SdePoint::new(24.0, 48.0, 96.0);
        point /= 2.0;
        assert_eq!(point, SdePoint::new(12.0, 24.0, 48.0));
    }

    #[test]
    fn sdepoint_mul_f64() {
        let product = SdePoint::new(1.0, -2.0, 3.0) * 2.5;
        assert_eq!(product, SdePoint::new(2.5, -5.0, 7.5));
    }

    #[test]
    fn sdepoint_div_f64() {
        let quotient = SdePoint::new(10.0, -20.0, 30.0) / 4.0;
        assert_eq!(quotient, SdePoint::new(2.5, -5.0, 7.5));
    }
}
