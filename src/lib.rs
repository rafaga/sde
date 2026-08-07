#![crate_name = "sde"]
//! Read Eve Online's SDE data from sqlite database
//!
//! Provides an abstraction layer over SDE data .
//! When the abstraction is used makes it fast to search
//! there are these advantages:
//!
//!
use crate::objects::{
    Constellation, MapPoint, MapSegment, Moon, Planet, Region, SolarSystem, Universe,
};
use kdtree::KdTree;
use objects::EveRegionArea;
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
    /// Creates a new SdeManager using a path to build the connection
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
    /// `get_abstract_systems`/`get_abstract_connections` don't (that was
    /// also the case in the previous code, before this migration).
    fn scale_coords(&self, mut coords: [f32; 2], invert: bool) -> [f32; 2] {
        if self.factor > 1 {
            let f = self.factor as f32;
            coords[0] /= f;
            coords[1] /= f;
        } else if self.factor < -1 {
            let f = self.factor.unsigned_abs() as f32;
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

    /// Method that retrieve all Eve Online universe data and some dictionaries to quick
    /// access the available data.
    ///
    /// Data retrieved:
    ///
    /// - Regions
    /// - Constellations
    /// - Solar Systems
    pub fn get_universe(&mut self) -> Result<bool, Error> {
        let filter = Vec::new();
        self.universe.regions = self.get_region(filter.clone(), None)?;
        self.universe.constellations = self.get_constellation(filter.clone())?;
        self.universe.solar_systems = self.get_solarsystem(filter)?;
        Ok(true)
    }

    /// Function to get all the K-Space solar systems coordinates from the SDE including data to build a map
    /// and search for basic stuff
    pub fn get_systempoints(&self) -> Result<KdTree<f64, MapPoint, [f64; 3]>, Error> {
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
        let mut point = MapPoint {
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
                let x = row.get::<usize, f32>(1)?;
                let y = row.get::<usize, f32>(2)?;

                //we get the coordinate point and multiply with the adjust factor
                let [x, y] = self.scale_coords([x, y], self.invert_coordinates);
                point = MapPoint {
                    id: Some(id.try_into().unwrap()),
                    name: Some(row.get::<usize, String>(3)?),
                    coords: [x as f64, y as f64, 0.0],
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

    pub fn get_region_coordinates(&self) -> Result<Vec<EveRegionArea>, Error> {
        let connection = self.get_standart_connection()?;

        let mut query = String::from("SELECT reg.regionId, reg.regionName, ");
        query += "MAX(reg.max_x) AS region_max_x, MAX(reg.max_y) AS region_max_y, ";
        query += "MIN(reg.min_x) AS region_min_x, MIN(reg.min_y) AS region_min_y ";
        query += "FROM (SELECT mr.regionId, mr.regionName, ";
        query += "mc.constellationId, MAX(mss.position2DX) AS max_x, MAX(mss.position2DY) AS max_y, ";
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
            // so this has to be read as f64 first and cast explicitly.
            //
            // EveRegionArea.max/min stay `MapPoint` (3D) for API
            // stability, but the region bounding box is now 2D (there's
            // no third component to report anymore) -- the Z component is
            // just always 0.
            region.max = MapPoint::from([
                row.get::<usize, f64>(2)? as i64,
                row.get::<usize, f64>(3)? as i64,
                0,
            ]);
            region.min = MapPoint::from([
                row.get::<usize, f64>(4)? as i64,
                row.get::<usize, f64>(5)? as i64,
                0,
            ]);
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

    pub fn get_system_coords(&self, id_node: usize) -> Result<Option<MapPoint>, Error> {
        let connection = self.get_standart_connection()?;

        // projX/Y/Z no longer exist (see the note in get_systempoints());
        // this function returns a genuinely 3D MapPoint (unlike
        // get_systempoints()/get_connections(), which only need 2
        // components), so it's migrated to centerX/Y/Z -- the system's
        // real 3D coordinates, always `NOT NULL` in the schema, without
        // the null-handling complexity position2DX/Y has.
        let mut query = String::from("SELECT mss.centerX, mss.centerY, mss.centerZ ");
        query += "FROM mapSolarSystems AS mss WHERE mss.SolarSystemId = ?1; ";

        let mut statement = connection.prepare(query.as_str())?;
        let system_like_name = id_node.to_string();
        let mut rows = statement.query(params![system_like_name])?;
        if let Some(row) = rows.next()? {
            let mut coord = MapPoint::from([
                row.get::<usize, f32>(0)?,
                row.get::<usize, f32>(1)?,
                row.get::<usize, f32>(2)?,
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

    pub fn get_connections(&self) -> Result<Vec<MapSegment>, Error> {
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
                [row.get::<usize, f32>(2)?, row.get::<usize, f32>(3)?],
                self.invert_coordinates,
            );
            let point2 = self.scale_coords(
                [row.get::<usize, f32>(4)?, row.get::<usize, f32>(5)?],
                self.invert_coordinates,
            );
            let id = (
                row.get::<usize, i64>(0)? as usize,
                row.get::<usize, i64>(1)? as usize,
            );
            results.push(MapSegment { id, point1, point2 });
        }
        Ok(results)
    }

    pub fn get_abstract_systems(
        &self,
        regions: Vec<u32>,
    ) -> Result<KdTree<f64, MapPoint, [f64; 3]>, Error> {
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
        let mut point = MapPoint {
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
                // get_abstract_systems doesn't invert coordinates -- that
                // was also the case before this migration (unlike
                // get_systempoints/get_connections, which do).
                let [x, y] = self.scale_coords(
                    [row.get::<usize, f32>(1)?, row.get::<usize, f32>(2)?],
                    false,
                );
                point = MapPoint {
                    id: Some(id.try_into().unwrap()),
                    name: Some(row.get::<usize, String>(6)?),
                    coords: [x as f64, y as f64, 0.0],
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

    pub fn get_abstract_connections(&self, regions: Vec<u32>) -> Result<Vec<MapSegment>, Error> {
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
                [row.get::<usize, f32>(2)?, row.get::<usize, f32>(3)?],
                false,
            );
            let point2 = self.scale_coords(
                [row.get::<usize, f32>(4)?, row.get::<usize, f32>(5)?],
                false,
            );
            let id = (
                row.get::<usize, i64>(0)? as usize,
                row.get::<usize, i64>(1)? as usize,
            );
            results.push(MapSegment { id, point1, point2 });
        }
        Ok(results)
    }

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

    fn get_solarsystem(&self, constellation: Vec<u32>) -> Result<HashMap<u32, SolarSystem>, Error> {
        // preparing the connections that will be shared between threads
        let connection = self.get_standart_connection()?;
        let mut result = HashMap::new();

        let mut query =
            String::from("SELECT mss.solarSystemId, mss.solarSystemName, mc.regionId, ");
        query += " mc.centerX, mc.centerY, mc.centerZ, mss.position2DX, mss.position2DY, ";
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
            object.real_coords.x = row.get::<_, f64>(3)? as i64; //i64
            object.real_coords.y = row.get::<_, f64>(4)? as i64; //i64
            object.real_coords.z = row.get::<_, f64>(5)? as i64; //i64
            // Unlike get_systempoints()/get_connections() (which filter
            // out systems without a 2D projection), the row is kept
            // as-is here: this method feeds general system data (name,
            // region, constellation, real coordinates), not just the
            // map, so a missing position2D falls back to (0.0, 0.0)
            // instead of excluding the system entirely.
            object.projected_coords.x = row.get::<_, Option<f64>>(6)?.unwrap_or(0.0) as i64;
            object.projected_coords.y = row.get::<_, Option<f64>>(7)?.unwrap_or(0.0) as i64;

            // Invert coordinates if needed
            if self.invert_coordinates {
                object.real_coords.x *= -1;
                object.real_coords.y *= -1;
                object.real_coords.z *= -1;
                object.projected_coords.x *= -1;
                object.projected_coords.y *= -1;
            }
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
        Ok(result)
    }

    /// Function to get every Constellation or a Constellation based on an specific Region
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

    /// Function to get every Planet or all Planets for a specific Solar System
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

    /// Function to get every Moon or all Moons for a specific planet
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
