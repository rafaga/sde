#![crate_name = "sde"]
//! Read Eve Online's SDE data from sqlite database
//!
//! Provides an abstraction layer over SDE data .
//! When the abstraction is used makes it fast to search
//! there are these advantages:
//!
//!
use crate::objects::{
    Constellation, SdePoint, SdeSegment, Moon, Planet, Region, SolarSystem, Universe,
};
use kdtree::KdTree;
use objects::EveRegionArea;
use rstar::RTree;
use rusqlite::ToSql;
use rusqlite::{Connection, Error, OpenFlags, params, vtab::array};
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;

/// Module that has Data object abstractions to fill with the database data.
pub mod objects;

/// Logic to (re)generate `sde.db` (feature `builder`, disabled by
/// default). See `src/builder/mod.rs` for the detail.
#[cfg(feature = "builder")]
pub mod builder;

/// Manages the process of reading SDE data and putting into different data structures
/// for easy in-memory access.
#[derive(Clone)]
pub struct SdeManager<'a> {
    /// The path to the SDE database
    pub path: &'a Path,
    /// The universe Object that contains all the data
    pub universe: Universe,
    /// Adjusting factor for coordinates (because are very large numbers)
    pub factor: i64,
    /// Invert the sign of all coordinate values
    pub invert_coordinates: bool,
}

impl<'a> SdeManager<'a> {
    /// Creates a new `SdeManager` pointing at the SQLite database at
    /// `path` (not opened yet -- each method opens its own connection
    /// via `Self::get_standart_connection` when it actually needs
    /// one). `factor` is the coordinate-scaling divisor/multiplier used
    /// throughout (see `Self::scale_coords`); it's also passed to
    /// [`objects::Universe::new`] to build the initial, empty
    /// `universe`. `invert_coordinates` starts `true`.
    pub fn new(path: &Path, factor: i64) -> SdeManager<'_> {
        SdeManager {
            path,
            universe: Universe::new(factor),
            factor, // 10000000000000
            invert_coordinates: true,
        }
    }

    /// Applies the adjustment factor (`self.factor`) and, if `invert` is
    /// `true`, flips the sign of both components. Replaces the
    /// `DivAssign`/`MulAssign` operators `egui_map::RawPoint` used to
    /// provide -- same logic as always (divide if the factor is > 1,
    /// multiply by its absolute value if it's < -1), now directly on
    /// `[f32; 2]`. `invert` is a parameter (not always
    /// `self.invert_coordinates`) because not every function calling
    /// this helper inverts: `get_systempoints`/`get_connections` do,
    /// `get_abstract_systems`/`get_abstract_connections` don't.
    ///
    /// Used by [`Self::get_systempoints`]/[`Self::get_abstract_systems`]
    /// (which build [`objects::SdePoint`], `coords: [f64; 3]`) and
    /// [`Self::get_connections`]/[`Self::get_abstract_connections`]
    /// (building [`objects::SdeSegment`], `point1`/`point2: [f64; 2]`) --
    /// both types are `f64` throughout, so there's a single version of
    /// this helper, not one per type as there used to be.
    ///
    /// Operating in `f32` anywhere along this path would silently
    /// reintroduce precision loss that reading the columns as `f64` (and
    /// scaling in `f64`) is meant to avoid: `some_f32_value as f64`
    /// doesn't recover the precision lost when the value was first
    /// narrowed to `f32` -- e.g. `0.1_f32 as f64` is
    /// `0.10000000149011612`, not `0.1`. `position2DX`/`position2DY`/
    /// `mapAbstractSystems.x`/`.y` are all `REAL` (i.e. already
    /// `f64`-precision in SQLite), so reading them directly as `f64`
    /// costs nothing and avoids that round-trip entirely.
    fn scale_coords(&self, mut coords: [f64; 2], invert: bool) -> [f64; 2] {
        if self.factor > 1 {
            let f = self.factor as f64;
            coords[0] /= f;
            coords[1] /= f;
        } else if self.factor < -1 {
            let f = self.factor.unsigned_abs() as f64;
            coords[0] *= f;
            coords[1] *= f;
        }
        if invert {
            coords[0] *= -1.0;
            coords[1] *= -1.0;
        }
        coords
    }

    /// Wraps a `kdtree::ErrorKind` (from a `KdTree::add` call) as a
    /// `rusqlite::Error`, so it can be propagated with `?` from
    /// functions that return `Result<_, rusqlite::Error>` -- these are
    /// two entirely different error crates, with no native conversion
    /// between them. Uses `ToSqlConversionFailure` (available without
    /// feature gating, unlike `ModuleError`, which requires the `vtab`
    /// feature) as the variant `rusqlite` itself intends for wrapping
    /// any foreign error -- even though its name suggests it's only for
    /// `ToSql` implementors.
    ///
    /// In practice this should never actually fire (`KdTree::add` only
    /// fails on `ZeroCapacity`, impossible with `KdTree::new`, or on
    /// non-finite coordinates, and `position2DX`/`position2DY` come
    /// filtered with `IS NOT NULL` in the query) -- it's still
    /// propagated as an error rather than assumed impossible with
    /// `expect`, because here there IS a theoretical path for corrupt
    /// data (a `NaN` stored in the database) that doesn't exist in
    /// `map_points_to_vec`.
    fn kdtree_error(err: kdtree::ErrorKind) -> Error {
        Error::ToSqlConversionFailure(Box::new(err))
    }

    /// Populates `self.universe` with the whole SDE: regions,
    /// constellations, solar systems, planets, and moons, all
    /// unfiltered (an empty id list to each underlying getter means "no
    /// filter, return everything" throughout this API). `planets`/
    /// `moons` come back from [`Self::get_planet`]/[`Self::get_moon`]
    /// as a `Vec`, keyed here by `.id` into the `HashMap` shape
    /// `universe.planets`/`.moons` actually store. Always returns
    /// `Ok(true)` -- the `bool` carries no information beyond
    /// "succeeded" (any failure short-circuits via `?` instead).
    pub fn get_universe(&mut self) -> Result<bool, Error> {
        let filter = Vec::new();
        self.universe.regions = self.get_region(filter.clone(), None)?;
        self.universe.constellations = self.get_constellation(filter.clone())?;
        self.universe.solar_systems = self.get_solarsystem(filter.clone())?;
        self.universe.planets = self
            .get_planet(filter.clone())?
            .into_iter()
            .map(|planet| (planet.id, planet))
            .collect();
        self.universe.moons = self
            .get_moon(filter)?
            .into_iter()
            .map(|moon| (moon.id, moon))
            .collect();
        Ok(true)
    }

    /// All K-space solar systems with a computed 2D map projection,
    /// indexed in a [`kdtree::KdTree`] by `[x, y, 0.0]` (the third
    /// component unused, kept for API consistency with
    /// [`objects::SdePoint`]'s 3D shape) -- built for nearest-neighbor
    /// queries, e.g. hit-testing a mouse click on a rendered map.
    ///
    /// "K-space" here means `solarSystemId` between `30000000` and
    /// `30999999`, a hardcoded range rather than a query against
    /// `ParserConfig`'s k-space/w-space/abyssal/void flags (those only
    /// affect what [`builder::parser`] writes at build time, not what
    /// this read-side method selects). Systems without a 2D projection
    /// (`position2DX`/`position2DY` both `NULL` -- CCP doesn't provide
    /// one for every system, and [`builder::parser`] only computes one
    /// locally when `force_isometric_position_2d` is set) are excluded
    /// entirely rather than appearing with a placeholder position.
    ///
    /// Each point also carries the ids of every solar system it has a
    /// stargate connection to (via `mapSystemConnections`), in
    /// [`objects::SdePoint::connections`].
    pub fn get_systempoints(&self) -> Result<KdTree<f64, SdePoint, [f64; 3]>, Error> {
        let connection = self.get_standart_connection()?;

        let mut tree = KdTree::new(3);
        // centerX, centerY, centerZ,
        let mut query = String::from(
            "SELECT sos.SolarSystemId, sos.position2DX, sos.position2DY, sos.SolarSystemName, msc.systemA, msc.systemB ",
        );
        query += " FROM mapSolarSystems AS sos RIGHT OUTER JOIN mapSystemConnections AS msc";
        query += " ON (msc.systemA = sos.SolarSystemId OR msc.systemB = sos.SolarSystemId)";
        query += " WHERE sos.SolarSystemId BETWEEN ?1 AND ?2";
        // position2DX/Y are nullable (unlike the old projX/Y/Z, which
        // always carried a value via DEFAULT(0.0)): a system without a
        // computed 2D projection (CCP doesn't provide one and local
        // computation wasn't forced, see
        // ParserConfig::force_isometric_position_2d) simply doesn't show
        // up on the map, instead of breaking the query.
        query += " AND sos.position2DX IS NOT NULL AND sos.position2DY IS NOT NULL";
        query += " ORDER BY sos.SolarSystemId ASC";
        let mut statement = connection.prepare(query.as_str())?;
        let mut rows = statement.query(params![30000000, 30999999])?;
        let mut last_id = isize::MIN;
        let mut point = SdePoint {
            id: None,
            name: None,
            coords: [0.0, 0.0, 0.0],
            connections: Vec::new(),
        };
        while let Some(row) = rows.next()? {
            let id = row.get::<usize, isize>(0)?;
            if id != last_id {
                if last_id != isize::MIN {
                    tree.add(point.coords, point.clone())
                        .map_err(Self::kdtree_error)?;
                }
                last_id = id;
                let x = row.get::<usize, f64>(1)?;
                let y = row.get::<usize, f64>(2)?;

                //we get the coordinate point and multiply with the adjust factor
                let [x, y] = self.scale_coords([x, y], self.invert_coordinates);
                point = SdePoint {
                    id: Some(id.try_into().unwrap()),
                    name: Some(row.get::<usize, String>(3)?),
                    coords: [x, y, 0.0],
                    connections: Vec::new(),
                };
            }
            point.connections.push((
                row.get::<usize, i64>(4)? as usize,
                row.get::<usize, i64>(5)? as usize,
            ));
        }
        if last_id != isize::MIN {
            tree.add(point.coords, point).map_err(Self::kdtree_error)?;
        }
        Ok(tree)
    }

    /// The 2D bounding box (`EveRegionArea.max`/`.min`) of every
    /// K-space region (`regionId` between `10000000` and `10999999`),
    /// computed from the `MAX`/`MIN` of every solar system's
    /// `position2DX`/`position2DY` across all its constellations.
    /// Regions where every system lacks a 2D projection are excluded
    /// (there's no box to report); regions with at least one projected
    /// system still get a box even if others in it are missing one,
    /// since `MAX`/`MIN` ignore individual `NULL`s.
    ///
    /// If `self.invert_coordinates`, both corners get their sign
    /// flipped *and* swapped with each other -- flipping the sign alone
    /// would leave what used to be the maximum corner with the smaller
    /// (now negative) coordinates, so `max`/`min` would no longer
    /// actually describe the box's extremes without the swap.
    pub fn get_region_coordinates(&self) -> Result<Vec<EveRegionArea>, Error> {
        let connection = self.get_standart_connection()?;

        let mut query = String::from("SELECT reg.regionId, reg.regionName, ");
        query += "MAX(reg.max_x) AS region_max_x, MAX(reg.max_y) AS region_max_y, ";
        query += "MIN(reg.min_x) AS region_min_x, MIN(reg.min_y) AS region_min_y ";
        query += "FROM (SELECT mr.regionId, mr.regionName, ";
        query +=
            "mc.constellationId, MAX(mss.position2DX) AS max_x, MAX(mss.position2DY) AS max_y, ";
        query += "MIN(mss.position2DX) AS min_x, MIN(mss.position2DY) AS min_y ";
        query += "FROM mapRegions AS mr ";
        query += "INNER JOIN mapConstellations mc ON (mc.regionId = mr.regionId) ";
        query += "INNER JOIN mapSolarSystems mss ON (mc.constellationId = mss.constellationId) ";
        query += " WHERE mr.regionId BETWEEN 10000000 AND 10999999 GROUP BY mr.regionId, mr.regionName, mc.constellationId) ";
        query += "AS reg GROUP BY reg.regionId ";
        // position2DX/Y are nullable; MAX()/MIN() already ignore each
        // individual system's NULLs, but if EVERY system in a region
        // lacks a 2D projection, the final aggregate still comes out
        // NULL -- that region is excluded here instead of breaking the
        // row read (there's no bounding box to report for it).
        query += "HAVING MAX(reg.max_x) IS NOT NULL AND MAX(reg.max_y) IS NOT NULL ";
        query += "AND MIN(reg.min_x) IS NOT NULL AND MIN(reg.min_y) IS NOT NULL;";
        let mut statement = connection.prepare(query.as_str())?;
        let mut rows = statement.query([])?;
        let mut areas = Vec::new();
        while let Some(row) = rows.next()? {
            let mut region = EveRegionArea::new();
            region.region_id = row.get(0)?;
            region.name = row.get(1)?;
            // mapSolarSystems.position2DX/Y have REAL affinity, so
            // MAX()/MIN() over them (even doubly-aggregated through the
            // subquery) also yield REAL storage class -- rusqlite's `i64`
            // FromSql impl does NOT coerce a SQLite REAL into an integer,
            // so this has to be read as f64. Unlike before the
            // SdePoint/SdePoint merge, there's no need to narrow it to
            // `i64` afterward -- `SdePoint::from([f64; 3])` takes the
            // f64 values directly, without an unnecessary
            // f64 -> i64 -> f64 round-trip that would silently truncate
            // any fractional part.
            //
            // EveRegionArea.max/min stay `SdePoint` (3D) for API
            // stability, but the region bounding box is now 2D (there's
            // no third component to report anymore) -- the Z component is
            // just always 0.
            region.max =
                SdePoint::from([row.get::<usize, f64>(2)?, row.get::<usize, f64>(3)?, 0.0]);
            region.min =
                SdePoint::from([row.get::<usize, f64>(4)?, row.get::<usize, f64>(5)?, 0.0]);
            // we invert the coordinates and swap the min with the max
            if self.invert_coordinates {
                std::mem::swap(&mut region.max, &mut region.min);
                region.min *= -1i64;
                region.max *= -1i64;
            }
            areas.push(region);
        }
        Ok(areas)
    }

    /// Finds solar systems by a case-insensitive substring match on
    /// their name (`%name%`), returning every match as
    /// `(solarSystemId, solarSystemName, regionId, regionName)` --
    /// there can be more than one, and there's no k-space/w-space
    /// filter here (unlike [`Self::get_systempoints`]). The query does
    /// `LOWER(solarSystemName) LIKE ?1` without lowercasing `name`
    /// itself, but that's not a bug: SQLite's `LIKE` is already
    /// case-insensitive for ASCII by default, so the `LOWER()` on the
    /// column side is redundant, not load-bearing.
    pub fn get_system_id(
        &self,
        name: String,
    ) -> Result<Vec<(isize, String, isize, String)>, Error> {
        let connection = self.get_standart_connection()?;

        let mut query = String::from(
            "SELECT mss.SolarSystemId, mss.SolarSystemName, mr.RegionId, mr.regionName ",
        );
        query += "FROM mapSolarSystems AS mss ";
        query +=
            "INNER JOIN mapConstellations AS mc ON (mc.constellationId = mss.constellationId) ";
        query += "INNER JOIN mapRegions AS mr ON (mr.RegionId = mc.RegionId) ";
        query += "WHERE LOWER(mss.SolarSystemName) LIKE ?1; ";

        let mut statement = connection.prepare(query.as_str())?;
        let system_like_name = "%".to_string() + name.as_str() + "%";
        let mut rows = statement.query(params![system_like_name])?;
        let mut results = Vec::new();
        while let Some(row) = rows.next()? {
            results.push((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?));
        }
        Ok(results)
    }

    /// The real 3D coordinates (`centerX`/`Y`/`Z`, always `NOT NULL` in
    /// the schema, unlike the nullable `position2DX`/`Y` used
    /// elsewhere) of the solar system with id `id_node`, scaled by
    /// `self.factor` and sign-flipped if `self.invert_coordinates`.
    /// `Ok(None)` if no system has that id -- not an error. (The local
    /// variable holding the id as a string is misleadingly named
    /// `system_like_name`: despite the name, the query does an exact
    /// `= ?1` match, not a `LIKE`.)
    pub fn get_system_coords(&self, id_node: usize) -> Result<Option<SdePoint>, Error> {
        let connection = self.get_standart_connection()?;

        // projX/Y/Z no longer exist (see the note in get_systempoints());
        // this function returns a genuinely 3D SdePoint (unlike
        // get_systempoints()/get_connections(), which only need 2
        // components), so it reads centerX/Y/Z -- the system's
        // real 3D coordinates, always `NOT NULL` in the schema, without
        // the null-handling complexity position2DX/Y has.
        let mut query = String::from("SELECT mss.centerX, mss.centerY, mss.centerZ ");
        query += "FROM mapSolarSystems AS mss WHERE mss.SolarSystemId = ?1; ";

        let mut statement = connection.prepare(query.as_str())?;
        let system_like_name = id_node.to_string();
        let mut rows = statement.query(params![system_like_name])?;
        if let Some(row) = rows.next()? {
            let mut coord = SdePoint::from([
                row.get::<usize, f64>(0)?,
                row.get::<usize, f64>(1)?,
                row.get::<usize, f64>(2)?,
            ]);
            if self.factor > 1 {
                coord /= self.factor;
            } else if self.factor < -1 {
                coord *= self.factor.abs();
            }
            if self.invert_coordinates {
                coord *= -1i64;
            }
            return Ok(Some(coord));
        }
        Ok(None)
    }

    /// Line segments connecting solar systems via stargates, indexed in
    /// an [`rstar::RTree`] -- built for spatial queries like "which
    /// connections intersect this area of the map"
    /// (`locate_in_envelope_intersecting`) or "which connection is
    /// closest to this point" (`nearest_neighbor`, hit-testing a mouse
    /// click), rather than a plain linear scan.
    pub fn get_connections(&self) -> Result<RTree<SdeSegment>, Error> {
        let connection = self.get_standart_connection()?;

        let mut query = String::from("SELECT msc.systemA, msc.systemB, ");
        query += "mssa.position2DX, mssa.position2DY, mssb.position2DX, mssb.position2DY ";
        query += "FROM mapSystemConnections AS msc INNER JOIN mapSolarSystems AS mssa ";
        query += "ON(msc.systemA = mssa.solarSystemId) INNER JOIN mapSolarSystems AS mssb ";
        query += "ON(msc.systemB = mssb.solarSystemId) ";
        // Both endpoints need a valid 2D projection to be able to draw
        // the line; if either one is missing it (see the same
        // nullability note in get_systempoints), the whole connection is
        // skipped instead of failing the entire query.
        query += "WHERE mssa.position2DX IS NOT NULL AND mssa.position2DY IS NOT NULL ";
        query += "AND mssb.position2DX IS NOT NULL AND mssb.position2DY IS NOT NULL;";

        let mut statement = connection.prepare(query.as_str())?;
        let mut rows = statement.query([])?;
        let mut results = vec![];
        while let Some(row) = rows.next()? {
            let point1 = self.scale_coords(
                [row.get::<usize, f64>(2)?, row.get::<usize, f64>(3)?],
                self.invert_coordinates,
            );
            let point2 = self.scale_coords(
                [row.get::<usize, f64>(4)?, row.get::<usize, f64>(5)?],
                self.invert_coordinates,
            );
            let id = (
                row.get::<usize, i64>(0)? as usize,
                row.get::<usize, i64>(1)? as usize,
            );
            results.push(SdeSegment { id, point1, point2 });
        }
        Ok(RTree::bulk_load(results))
    }

    /// Same shape and purpose as [`Self::get_systempoints`], but for the
    /// abstract map (`mapAbstractSystems`, from `builder::community`'s
    /// community-maintained, third-party layer -- see
    /// `ParserConfig.with_third_party`) instead of the canonical one:
    /// every abstract system, optionally filtered to just the given
    /// `regions` (an empty `Vec` means no filter, same convention as
    /// every other filtered getter here), indexed in a
    /// [`kdtree::KdTree`] with each point's stargate connections
    /// attached. Unlike [`Self::get_systempoints`], coordinates are
    /// never inverted here regardless of `self.invert_coordinates`.
    ///
    /// `mapAbstractSystems` doesn't exist at all in a database built
    /// without `--with-third-party` (or, equivalently,
    /// `ParserConfig.with_third_party = false`) -- this method returns
    /// `Err(rusqlite::Error::SqliteFailure(..., "no such table:
    /// mapAbstractSystems"))` in that case, not a panic. There's
    /// currently no way to check for this ahead of the call other than
    /// handling that `Err`; a fingerprint of what a given database
    /// actually contains, queryable without hitting this error, is
    /// planned but not implemented yet.
    pub fn get_abstract_systems(
        &self,
        regions: Vec<u32>,
    ) -> Result<KdTree<f64, SdePoint, [f64; 3]>, Error> {
        let connection = self.get_standart_connection()?;

        let mut query = String::from("SELECT mas.solarSystemId, mas.x, mas.y, mas.regionId, ");
        query += "  msc.systemA, msc.systemB, mss.solarSystemName ";
        query += " FROM mapAbstractSystems AS mas RIGHT OUTER JOIN mapSystemConnections AS msc ";
        query += " ON(msc.systemA = mas.solarSystemId OR msc.systemB = mas.solarSystemId) ";
        query += " INNER JOIN mapSolarSystems AS mss ON (mss.solarSystemId = mas.solarSystemId) ";
        if !regions.is_empty() {
            query += " WHERE mas.regionId IN rarray(?1) ";
        }
        query += " ORDER BY mas.solarsystemId ASC;";

        let mut statement = connection.prepare(query.as_str())?;
        let mut rows;
        let mut tree = KdTree::new(3);

        if regions.is_empty() {
            rows = statement.query([])?;
        } else {
            let id_list: array::Array = Rc::new(
                regions
                    .into_iter()
                    .map(rusqlite::types::Value::from)
                    .collect::<Vec<rusqlite::types::Value>>(),
            );
            rows = statement.query([id_list])?;
        }

        let mut current_index = isize::MIN;
        let mut point = SdePoint {
            id: None,
            name: None,
            coords: [0.0, 0.0, 0.0],
            connections: Vec::new(),
        };
        while let Some(row) = rows.next()? {
            let id = row.get::<usize, isize>(0)?;
            if current_index != id {
                if current_index != isize::MIN {
                    tree.add(point.coords, point.clone())
                        .map_err(Self::kdtree_error)?;
                }
                current_index = id;
                // get_abstract_systems doesn't invert coordinates, unlike
                // get_systempoints/get_connections, which do.
                let [x, y] = self.scale_coords(
                    [row.get::<usize, f64>(1)?, row.get::<usize, f64>(2)?],
                    false,
                );
                point = SdePoint {
                    id: Some(id.try_into().unwrap()),
                    name: Some(row.get::<usize, String>(6)?),
                    coords: [x, y, 0.0],
                    connections: Vec::new(),
                };
            }
            point.connections.push((
                row.get::<usize, i64>(4)? as usize,
                row.get::<usize, i64>(5)? as usize,
            ));
        }
        if current_index != isize::MIN {
            tree.add(point.coords, point).map_err(Self::kdtree_error)?;
        }
        Ok(tree)
    }

    /// Same as [`Self::get_connections`], but for the abstract map
    /// (`mapAbstractSystems`), optionally filtered by region. Also
    /// indexed in an [`rstar::RTree`] for the same spatial-query
    /// reasons.
    ///
    /// Same caveat as [`Self::get_abstract_systems`]: fails with
    /// `Err(rusqlite::Error::SqliteFailure(..., "no such table:
    /// mapAbstractSystems"))`, not a panic, against a database built
    /// without `--with-third-party`.
    pub fn get_abstract_connections(&self, regions: Vec<u32>) -> Result<RTree<SdeSegment>, Error> {
        let connection = self.get_standart_connection()?;

        let mut query = String::from("SELECT msc.systemA, msc.systemB, ");
        query += "masa.x, masa.y, masb.x, masb.y ";
        query += "FROM mapSystemConnections AS msc INNER JOIN mapAbstractSystems AS masa ";
        query += "ON(msc.systemA = masa.solarSystemId) INNER JOIN mapAbstractSystems AS masb ";
        query += "ON(msc.systemB = masb.solarSystemId) ";
        if !regions.is_empty() {
            query += " WHERE masa.regionId IN rarray(?1) AND masb.regionId IN rarray(?2);";
        }

        let mut statement = connection.prepare(query.as_str())?;
        let mut rows;
        if regions.is_empty() {
            rows = statement.query([])?;
        } else {
            let id_list: array::Array = Rc::new(
                regions
                    .into_iter()
                    .map(rusqlite::types::Value::from)
                    .collect::<Vec<rusqlite::types::Value>>(),
            );
            rows = statement.query([id_list.clone(), id_list])?;
        }

        let mut results = vec![];
        while let Some(row) = rows.next()? {
            let point1 = self.scale_coords(
                [row.get::<usize, f64>(2)?, row.get::<usize, f64>(3)?],
                false,
            );
            let point2 = self.scale_coords(
                [row.get::<usize, f64>(4)?, row.get::<usize, f64>(5)?],
                false,
            );
            let id = (
                row.get::<usize, i64>(0)? as usize,
                row.get::<usize, i64>(1)? as usize,
            );
            results.push(SdeSegment { id, point1, point2 });
        }
        Ok(RTree::bulk_load(results))
    }

    /// Opens a fresh read connection to `self.path` with sensible
    /// defaults for this crate's read-only workload: the `rarray`
    /// virtual table module loaded (every filtered getter here passes
    /// its id list through it) and `PRAGMA foreign_keys = ON` for
    /// consistency with the rest of the codebase, even though it has no
    /// effect on `SELECT`-only queries. Called once per public method
    /// that needs the database -- there's no connection pooling or
    /// reuse across calls.
    fn get_standart_connection(&self) -> Result<Connection, Error> {
        let mut flags = OpenFlags::default();
        flags.set(OpenFlags::SQLITE_OPEN_NO_MUTEX, false);
        flags.set(OpenFlags::SQLITE_OPEN_FULL_MUTEX, true);
        let connection = Connection::open_with_flags(self.path, flags)?;

        // we add the carray module disguised as rarray in rusqlite
        array::load_module(&connection)?;

        // `SdeManager` only ever runs SELECTs, so `foreign_keys = ON` doesn't
        // change query results (FK enforcement only applies to writes), but
        // it's harmless to enable and keeps the connection consistent with
        // the rest of the codebase. `execute_batch` (unlike `prepare`, which
        // only compiles the *first* statement in the string) runs every
        // statement it's given, so this actually takes effect.
        //
        // NOTE: this used to also set `PRAGMA journal_mode=WAL` (with a typo,
        // "journey_mode", so it silently never ran). WAL is intentionally
        // NOT enabled here: it persists a mode change into the database file
        // itself and requires write access to the containing directory (to
        // create the `-wal`/`-shm` siblings) on *every* call to this method,
        // which would break any consumer shipping a read-only `sde.db`. If
        // you need WAL for a specific deployment, set it once out-of-band
        // (e.g. as part of the builder) rather than on every read connection.
        connection.execute_batch("PRAGMA foreign_keys = ON;")?;

        Ok(connection)
    }

    /// Every region, optionally narrowed by `regions` (an id allowlist)
    /// and/or `region_name` (a case-insensitive substring match, same
    /// `LIKE` caveat as [`Self::get_system_id`]) -- both empty/`None`
    /// means no filter; both given combines them with `AND`. Each
    /// returned [`objects::Region`] has its `constellations` populated
    /// (a second query, filtered to just the regions the first one
    /// matched).
    pub fn get_region(
        &self,
        regions: Vec<u32>,
        region_name: Option<String>,
    ) -> Result<HashMap<u32, Region>, Error> {
        let mut id_list: array::Array;
        let mut params: Vec<&dyn ToSql> = Vec::new();
        let mut _temp_value = String::new();
        let mut region_ids: Vec<u32> = Vec::new();

        let connection = self.get_standart_connection()?;
        let mut result = HashMap::new();

        let mut query = String::from("SELECT regionId, regionName FROM mapRegions ");
        if !regions.is_empty() || region_name.is_some() {
            let mut query_p = String::new();

            if !regions.is_empty() {
                query_p += "regionId IN rarray(?) ";
                id_list = Rc::new(
                    regions
                        .clone()
                        .into_iter()
                        .map(rusqlite::types::Value::from)
                        .collect::<Vec<rusqlite::types::Value>>(),
                );
                params.push(&id_list);
            }
            if region_name.is_some() {
                if !query_p.is_empty() {
                    query_p += " AND ";
                }
                query_p += "LOWER(regionName) LIKE ? ";
                _temp_value
                    .clone_from(&("%".to_string() + region_name.clone().unwrap().as_str() + "%"));
                params.push(&_temp_value);
            }
            if !query_p.is_empty() {
                query += &(" WHERE ".to_owned() + &query_p);
            }
        }
        query += "ORDER BY regionName ";

        let mut statement = connection.prepare(query.as_str())?;
        let mut rows;
        if params.is_empty() {
            rows = statement.query([])?;
        } else {
            rows = statement.query(params.as_slice())?;
        }

        while let Some(row) = rows.next()? {
            let mut region = Region::new();
            region.id = row.get(0)?;
            region.name = row.get(1)?;
            region_ids.push(row.get(0)?);
            result.insert(row.get(0)?, region);
        }

        let mut query = String::from("SELECT regionId,constellationId FROM mapConstellations");
        if !regions.is_empty() || region_name.is_some() {
            query += " WHERE regionId IN rarray(?1) ";
        }

        let mut statement = connection.prepare(query.as_str())?;
        let mut rows;

        if regions.is_empty() && region_name.is_none() {
            rows = statement.query([])?;
        } else {
            id_list = Rc::new(
                region_ids
                    .clone()
                    .into_iter()
                    .map(rusqlite::types::Value::from)
                    .collect::<Vec<rusqlite::types::Value>>(),
            );
            rows = statement.query([id_list])?;
        }

        while let Some(row) = rows.next()? {
            result
                .entry(row.get(0)?)
                .and_modify(|xregion| xregion.constellations.push(row.get(1).unwrap()));
        }
        Ok(result)
    }

    /// Every solar system, optionally narrowed to just the given
    /// `constellation` ids (empty means no filter), keyed by
    /// `solarSystemId`. Each [`objects::SolarSystem`] carries both its
    /// real 3D position (`real_coords`, from its own
    /// `centerX`/`Y`/`Z`) and its 2D map position (`projected_coords`,
    /// from `position2DX`/`Y`, falling back to `(0.0, 0.0)` if the
    /// system has none) plus its stargate `connections`,
    /// `disallowed_anchor_categories`, and `disallowed_anchor_groups`,
    /// each populated by its own second query (over
    /// `mapSystemConnections`/`mapSolarSystemDisallowedAnchorableCategories`/
    /// `...Groups` respectively) -- empty for the (large majority of)
    /// systems with no restrictions of that kind, populated for the
    /// ones that do. Unlike
    /// [`Self::get_systempoints`]/[`Self::get_connections`], systems
    /// without a 2D projection are kept (with that fallback position)
    /// rather than excluded -- this method feeds general system data,
    /// not just the map.
    fn get_solarsystem(&self, constellation: Vec<u32>) -> Result<HashMap<u32, SolarSystem>, Error> {
        // preparing the connections that will be shared between threads
        let connection = self.get_standart_connection()?;
        let mut result = HashMap::new();

        let mut query =
            String::from("SELECT mss.solarSystemId, mss.solarSystemName, mc.regionId, ");
        query += " mss.centerX, mss.centerY, mss.centerZ, mss.position2DX, mss.position2DY, ";
        query += " mss.constellationId FROM mapSolarSystems AS mss ";
        query +=
            " INNER JOIN mapConstellations AS mc ON(mss.constellationId = mc.constellationId)  ";
        if !constellation.is_empty() {
            query += " WHERE mss.constellationId IN rarray(?1);";
        }
        let mut statement = connection.prepare(query.as_str())?;

        let mut rows;
        if constellation.is_empty() {
            rows = statement.query([])?;
        } else {
            let id_list: array::Array = Rc::new(
                constellation
                    .into_iter()
                    .map(rusqlite::types::Value::from)
                    .collect::<Vec<rusqlite::types::Value>>(),
            );
            rows = statement.query([id_list])?;
        }

        while let Some(row) = rows.next()? {
            let mut object = SolarSystem::new(self.factor);
            object.id = row.get(0)?;
            object.name = row.get(1)?;
            object.constellation = row.get(8)?;

            let mut real_x = row.get::<_, f64>(3)?;
            let mut real_y = row.get::<_, f64>(4)?;
            let mut real_z = row.get::<_, f64>(5)?;
            // Unlike get_systempoints()/get_connections() (which filter
            // out systems without a 2D projection), the row is kept
            // as-is here: this method feeds general system data (name,
            // region, constellation, real coordinates), not just the
            // map, so a missing position2D falls back to (0.0, 0.0)
            // instead of excluding the system entirely.
            let mut proj_x = row.get::<_, Option<f64>>(6)?.unwrap_or(0.0);
            let mut proj_y = row.get::<_, Option<f64>>(7)?.unwrap_or(0.0);

            // Invert coordinates if needed
            if self.invert_coordinates {
                real_x *= -1.0;
                real_y *= -1.0;
                real_z *= -1.0;
                proj_x *= -1.0;
                proj_y *= -1.0;
            }
            object.real_coords = SdePoint::new(real_x, real_y, real_z);
            object.projected_coords = SdePoint::new(proj_x, proj_y, 0.0);

            object.region = row.get(2)?;
            result.insert(row.get(0)?, object);
        }

        let query = String::from("SELECT systemA, systemB FROM mapSystemConnections;");

        let mut statement = connection.prepare(query.as_str())?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            // Optimization: to avoid printing twice the same line, we are just skipping coordinates
            // for SolarSystems that has an Id less than the current one printed. with the exception
            // of the lowest ID
            let system_a = row.get::<usize, u32>(0)?;
            let system_b = row.get::<usize, u32>(1)?;

            //we compare the current system with the first, if not the same then we add the coordinates to hashmap
            result.entry(system_a).and_modify(|point| {
                point.connections.push(system_b);
            });

            result.entry(system_b).and_modify(|point| {
                point.connections.push(system_a);
            });
        }

        let query = String::from(
            "SELECT solarSystemId, categoryId FROM mapSolarSystemDisallowedAnchorableCategories;",
        );
        let mut statement = connection.prepare(query.as_str())?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let system_id = row.get::<usize, u32>(0)?;
            let category_id = row.get::<usize, u32>(1)?;
            result.entry(system_id).and_modify(|point| {
                point.disallowed_anchor_categories.push(category_id);
            });
        }

        let query = String::from(
            "SELECT solarSystemId, groupId FROM mapSolarSystemDisallowedAnchorableGroups;",
        );
        let mut statement = connection.prepare(query.as_str())?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let system_id = row.get::<usize, u32>(0)?;
            let group_id = row.get::<usize, u32>(1)?;
            result.entry(system_id).and_modify(|point| {
                point.disallowed_anchor_groups.push(group_id);
            });
        }

        Ok(result)
    }

    /// Every constellation, optionally narrowed to just the given
    /// `regions` (an id allowlist; empty means no filter). Each
    /// [`objects::Constellation`] has its `solar_systems` populated (a
    /// second query, filtered to just the constellations the first one
    /// matched) -- same two-query shape as [`Self::get_region`].
    fn get_constellation(&self, regions: Vec<u32>) -> Result<HashMap<u32, Constellation>, Error> {
        // preparing the connections that will be shared between threads
        let connection = self.get_standart_connection()?;
        let mut result = HashMap::new();
        let mut constellations = Vec::new();

        let mut query = String::from("SELECT constellationId, constellationName, regionId ");
        query += "FROM mapConstellations ";
        if !regions.is_empty() {
            query += "WHERE regionId IN rarray(?1);";
        }

        let mut statement = connection.prepare(query.as_str())?;
        let mut rows;
        if regions.is_empty() {
            rows = statement.query([])?;
        } else {
            let id_list: array::Array = Rc::new(
                regions
                    .into_iter()
                    .map(rusqlite::types::Value::from)
                    .collect::<Vec<rusqlite::types::Value>>(),
            );
            rows = statement.query([id_list])?;
        }

        //while there are regions left to consume
        while let Some(row) = rows.next()? {
            let mut object = Constellation::new();
            object.id = row.get(0)?;
            object.name = row.get(1)?;
            object.region = row.get(2)?;
            constellations.push(row.get::<usize, u32>(0)?);
            result.insert(row.get(0)?, object);
        }

        let mut query = String::from("SELECT constellationId, solarSystemId FROM mapSolarSystems");
        query += " WHERE constellationId IN rarray(?1);";

        let mut statement = connection.prepare(query.as_str())?;
        let id_list = Rc::new(
            constellations
                .into_iter()
                .map(rusqlite::types::Value::from)
                .collect::<Vec<rusqlite::types::Value>>(),
        );
        let mut rows = statement.query(params![id_list])?;

        while let Some(row) = rows.next()? {
            result
                .entry(row.get(0)?)
                .and_modify(|constel| constel.solar_systems.push(row.get(1).unwrap()));
        }

        Ok(result)
    }

    /// Every planet, optionally narrowed to just the given
    /// `solar_systems` (an id allowlist; empty means no filter). Unlike
    /// [`Self::get_region`]/`Self::get_constellation`/
    /// `Self::get_solarsystem`, this returns a flat `Vec`, not a
    /// `HashMap` keyed by id -- [`Self::get_universe`] keys it into one
    /// itself when populating `universe.planets`.
    pub fn get_planet(&self, solar_systems: Vec<u32>) -> Result<Vec<Planet>, Error> {
        // preparing the connections that will be shared between threads
        let connection = self.get_standart_connection()?;
        let mut result = vec![];

        let mut query = String::from("SELECT planetId, planetaryIndex, solarSystemId");
        query += " FROM mapPlanets";
        if !solar_systems.is_empty() {
            query += " WHERE solarSystemId IN rarray(?1)";
        }

        let mut statement = connection.prepare(query.as_str())?;
        let mut rows;
        if solar_systems.is_empty() {
            rows = statement.query([])?;
        } else {
            let id_list: array::Array = Rc::new(
                solar_systems
                    .into_iter()
                    .map(rusqlite::types::Value::from)
                    .collect::<Vec<rusqlite::types::Value>>(),
            );
            rows = statement.query([id_list])?;
        }

        //while there are regions left to consume
        while let Some(row) = rows.next()? {
            let mut object = Planet::new();
            object.id = row.get(0)?;
            object.solar_system = row.get(2)?;
            object.index = row.get(1)?;
            result.push(object);
        }

        Ok(result)
    }

    /// Every moon, optionally narrowed to just the given `planets` (an
    /// id allowlist; empty means no filter). Same flat-`Vec` shape as
    /// [`Self::get_planet`] -- [`Self::get_universe`] keys it into a
    /// `HashMap` itself when populating `universe.moons`.
    pub fn get_moon(&self, planets: Vec<u32>) -> Result<Vec<Moon>, Error> {
        // preparing the connections that will be shared between threads
        let connection = self.get_standart_connection()?;
        let mut result = vec![];

        let mut query = String::from("SELECT moonId, moonIndex, solarSystemId, planetId ");
        query += "FROM mapMoons ";

        if !planets.is_empty() {
            query += " WHERE planetId IN rarray(?1)";
        };

        let mut statement = connection.prepare(query.as_str())?;
        let mut rows;
        if planets.is_empty() {
            rows = statement.query([])?;
        } else {
            let id_list: array::Array = Rc::new(
                planets
                    .into_iter()
                    .map(rusqlite::types::Value::from)
                    .collect::<Vec<rusqlite::types::Value>>(),
            );
            rows = statement.query([id_list])?;
        }
        //while there are regions left to consume
        while let Some(row) = rows.next()? {
            let mut object = Moon::new();
            object.id = row.get(0)?;
            object.planet = row.get(3)?;
            object.index = row.get(1)?;
            object.solar_system = row.get(2)?;
            result.push(object);
        }

        Ok(result)
    }
}
