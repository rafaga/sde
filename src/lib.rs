#![crate_name = "sde"]
//! Read Eve Online's SDE data from sqlite database
//!
//! Provides an abstraction layer over SDE data .
//! When the abstraction is used makes it fast to search
//! there are these advantages:
//!
//!
use crate::objects::{SdePoint, Universe};
use egui_map::map::objects::{MapLine, MapPoint,RawPoint};
use objects::EveRegionArea;
use rusqlite::{params, Connection, Error, OpenFlags};
use std::collections::HashMap;
use std::path::Path;

/// Module that has Data object abstractions to fill with the database data.
pub mod objects;

/// Module that contains some hardcoded values useful to the crate
pub mod consts {

    /// Maximum number of threads to invoke in a multithread routines
    pub const MAX_THREADS: i8 = 8;
}

/// Manages the process of reading SDE data and putting into different data structures
/// for easy in-memory access.
#[derive(Clone)]
pub struct SdeManager<'a> {
    /// The path to the SDE database
    pub path: &'a Path,
    /// The universe Object that contains all the data
    pub universe: Universe,
    /// Adjusting factor for coordinates (because are very large numbers)
    pub factor: u64,
    /// Invert the sign of all coordinate values
    pub invert_coordinates: bool,
}

impl<'a> SdeManager<'a> {
    /// Creates a new SdeManager using a path to build the connection
    pub fn new(path: &Path, factor: u64) -> SdeManager {
        SdeManager {
            path,
            universe: Universe::new(factor),
            factor, // 10000000000000
            invert_coordinates: true,
        }
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
        #[cfg(feature = "puffin")]
        puffin::profile_scope!("get_universe");

        let mut flags = OpenFlags::default();
        flags.set(OpenFlags::SQLITE_OPEN_NO_MUTEX, false);
        flags.set(OpenFlags::SQLITE_OPEN_FULL_MUTEX, true);
        let connection = Connection::open_with_flags(self.path, flags)?;
        //let connection = sqlite::Connection::open_with_full_mutex(self.path)?;
        // Getting all regions in the main process because is not very intensive
        let regions = self.universe.get_region(&connection, None)?;
        let mut parent_ids = vec![];

        // fill a vector with ids to get constellations, fill a Hashmap of regions and dictionary to find fast an id
        for region in regions {
            #[cfg(feature = "puffin")]
            puffin::profile_scope!("hashmap_add_regions");
            parent_ids.push(region.id);
            self.universe
                .dicts
                .region_names
                .entry(region.name.to_lowercase().clone())
                .or_insert(region.id);
            self.universe.regions.entry(region.id).or_insert(region);
        }

        let parent_ids = match parent_ids.len() {
            x if x > 0 => Some(parent_ids),
            _ => None,
        };

        let constellations = self.universe.get_constellation(connection, parent_ids)?;

        let mut parent_ids = vec![];
        for constel in constellations {
            #[cfg(feature = "puffin")]
            puffin::profile_scope!("hashmap_add_constellations");
            parent_ids.push(constel.id);
            self.universe
                .dicts
                .constellation_names
                .entry(constel.name.to_lowercase().clone())
                .or_insert(constel.id);
            self.universe
                .constellations
                .entry(constel.id)
                .or_insert(constel);
        }

        let parent_ids = match parent_ids.len() {
            x if x > 0 => Some(parent_ids),
            _ => None,
        };

        let mut flags = OpenFlags::default();
        flags.set(OpenFlags::SQLITE_OPEN_NO_MUTEX, false);
        flags.set(OpenFlags::SQLITE_OPEN_FULL_MUTEX, true);
        let connection = Connection::open_with_flags(self.path, flags)?;
        let solar_systems =
            self.universe
                .get_solarsystem(connection, parent_ids, self.invert_coordinates)?;

        let mut parent_ids = vec![];
        for system in solar_systems {
            #[cfg(feature = "puffin")]
            puffin::profile_scope!("hashmap_add_planetary_systems");
            parent_ids.push(system.id);
            self.universe
                .dicts
                .constellation_names
                .entry(system.name.to_lowercase().clone())
                .or_insert(system.id);
            self.universe
                .solar_systems
                .entry(system.id)
                .or_insert(system);
        }
        Ok(true)
    }

    /// Function to get all the K-Space solar systems coordinates from the SDE including data to build a map
    /// and search for basic stuff
    pub fn get_systempoints(&self) -> Result<HashMap<usize, MapPoint>, Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_scope!("get_systempoints");

        let mut hash_map: HashMap<usize, MapPoint> = HashMap::new();
        let mut flags = OpenFlags::default();
        flags.set(OpenFlags::SQLITE_OPEN_NO_MUTEX, false);
        flags.set(OpenFlags::SQLITE_OPEN_FULL_MUTEX, true);
        let connection = Connection::open_with_flags(self.path, flags)?;

        // centerX, centerY, centerZ,
        let mut query = String::from("SELECT SolarSystemId, projX, projY, projZ, SolarSystemName ");
        query += " FROM mapSolarSystems WHERE SolarSystemId BETWEEN ?1 AND ?2;";
        let mut statement = connection.prepare(query.as_str())?;
        let mut rows = statement.query(params![30000000,30999999])?;
        let mut min_id = usize::MAX;
        while let Some(row) = rows.next()? {
            let id= row.get(0)?;
            if id < min_id {
                min_id = id;
            }
            let x = row.get::<usize,f32>(1)?;
            let y = row.get::<usize,f32>(2)?;
            let z = row.get::<usize,f32>(3)?;

            //we get the coordinate point and multiply with the adjust factor
            let mut coord = SdePoint::from([x as i64,y as i64,z as i64]);
            coord /= self.factor;
            if self.invert_coordinates {
                coord *= -1;
            }
            let mut point = MapPoint::new(id, coord.into());
            point.set_name(row.get::<usize,String>(4)?);
            hash_map.insert(id, point);
        }
        Ok(hash_map)
    }

    pub fn get_system_connections(
        &self,
        mut hash_map: HashMap<usize, MapPoint>,
    ) -> Result<HashMap<usize, MapPoint>, Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_scope!("get_system_connections");

        let mut flags = OpenFlags::default();
        flags.set(OpenFlags::SQLITE_OPEN_NO_MUTEX, false);
        flags.set(OpenFlags::SQLITE_OPEN_FULL_MUTEX, true);
        let connection = Connection::open_with_flags(self.path, flags)?;

        let mut query = String::from("SELECT systemConnectionId, ");
        query += "systemA, systemB FROM mapSystemConnections;";

        let mut statement = connection.prepare(query.as_str())?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            // Optimization: to avoid printing twice the same line, we are just skipping coordinates
            // for SolarSystems that has an Id less than the current one printed. with the exception
            // of the lowest ID
            let id  = row.get::<usize, String>(0)?;
            let system_a = row.get::<usize, usize>(1)?;
            let system_b = row.get::<usize, usize>(2)?;

            //we compare the current system with the first, if not the same then we add the coordinates to hashmap

            hash_map
                .entry(system_a)
                .and_modify(|point|{
                    point.connections.push(id.clone());
                });
            
            hash_map
                .entry(system_b)
                .and_modify(|point|{
                    point.connections.push(id);
                });
        }
        Ok(hash_map)
    }

    pub fn get_region_coordinates(&self) -> Result<Vec<EveRegionArea>, Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_scope!("get_region_coordinates");
        let mut flags = OpenFlags::default();
        flags.set(OpenFlags::SQLITE_OPEN_NO_MUTEX, false);
        flags.set(OpenFlags::SQLITE_OPEN_FULL_MUTEX, true);
        let connection = Connection::open_with_flags(self.path, flags)?;
        let mut query = String::from("SELECT reg.regionId, reg.regionName, ");
        query += "AX(reg.max_x) AS region_max_x, MAX(reg.max_y) AS region_max_y, ";
        query += "MAX(reg.max_z) AS region_max_z, MIN(reg.min_x) AS region_min_x, ";
        query += "MIN(reg.min_y) AS region_min_y, MIN(reg.min_z) AS region_min_z ";
        query += "FROM (SELECT mr.regionId, mr.regionName, ";
        query += "mc.constellationId, MAX(mss.projX) AS max_x, MAX(mss.projY) AS max_y, ";
        query += "MAX(mss.projZ) AS max_z, MIN(mss.projX) AS min_x, MIN(mss.projY) AS min_y, ";
        query += "MIN(mss.projZ) AS min_z FROM mapRegions AS mr ";
        query += "INNER JOIN mapConstellations mc ON (mc.regionId = mr.regionId) ";
        query += "INNER JOIN mapSolarSystems mss ON (mc.constellationId = mss.constellationId) ";
        query += " WHERE mr.regionId BETWEEN 10000000 AND 10999999 GROUP BY mr.regionId, mr.regionName, mc.constellationId) ";
        query += "AS reg GROUP BY reg.regionId;";
        let mut statement = connection.prepare(query.as_str())?;
        let mut rows = statement.query([])?;
        let mut areas = Vec::new();
        while let Some(row) = rows.next()? {
            let mut region = EveRegionArea::new();
            region.region_id = row.get(0)?;
            region.region_id = row.get(1)?;
            region.max = SdePoint::from([row.get::<usize,i64>(2)?,row.get::<usize,i64>(3)?,row.get::<usize,i64>(4)?]);
            region.min = SdePoint::from([row.get::<usize,i64>(5)?,row.get::<usize,i64>(6)?,row.get::<usize,i64>(7)?]);
            // we invert the coordinates and swap the min with the max
            if self.invert_coordinates {
                let temp = region.max;
                region.max = region.min;
                region.min = temp;
                region.min *= -1;
                region.max *= -1;
            }
            areas.push(region);
        }
        Ok(areas)
    }

    pub fn get_system_id(self, name: String) -> Result<Vec<(usize, String, usize, String)>, Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_scope!("get_system_id");
        let mut flags = OpenFlags::default();
        flags.set(OpenFlags::SQLITE_OPEN_NO_MUTEX, false);
        flags.set(OpenFlags::SQLITE_OPEN_FULL_MUTEX, true);
        let connection = Connection::open_with_flags(self.path, flags)?;

        let mut query = String::from(
            "SELECT mss.SolarSystemId, mss.SolarSystemName, mr.RegionId, mr.regionName ",
        );
        query += "FROM mapSolarSystems AS mss ";
        query += "INNER JOIN mapConstellations AS mc ON (mc.constellationId = mss.constellationId) ";
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

    pub fn get_system_coords(self, id_node: usize) -> Result<Option<SdePoint>, Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_scope!("get_system_coords");
        let mut flags = OpenFlags::default();
        flags.set(OpenFlags::SQLITE_OPEN_NO_MUTEX, false);
        flags.set(OpenFlags::SQLITE_OPEN_FULL_MUTEX, true);
        let connection = Connection::open_with_flags(self.path, flags)?;

        let mut query = String::from("SELECT mss.ProjX, mss.ProjY, mss.ProjZ ");
        query += "FROM mapSolarSystems AS mss WHERE mss.SolarSystemId = ?1; ";

        let mut statement = connection.prepare(query.as_str())?;
        let system_like_name = id_node.to_string();
        let mut rows = statement.query(params![system_like_name])?;
        if let Some(row) = rows.next()? {
            let mut coord = SdePoint::from([row.get::<usize, i64>(0)?,row.get::<usize, i64>(1)?,row.get::<usize, i64>(2)?]);
            coord /= self.factor;
            if self.invert_coordinates {
                coord *= -1;
            }
            return Ok(Some(coord));
        }
        Ok(None)
    }

    pub fn get_connections(self) -> Result<HashMap<String, MapLine>, Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_scope!("get_connections");

        let mut flags = OpenFlags::default();
        flags.set(OpenFlags::SQLITE_OPEN_NO_MUTEX, false);
        flags.set(OpenFlags::SQLITE_OPEN_FULL_MUTEX, true);
        let connection = Connection::open_with_flags(self.path, flags)?;

        let mut query = String::from("SELECT msc.systemConnectionId, ");
        query += "mssa.projX, mssa.projY, mssa.projZ, mssb.projX, mssb.projY, mssb.projZ ";
        query += "FROM mapSystemConnections AS msc INNER JOIN mapSolarSystems AS mssa ";
        query += "ON(msc.systemA = mssa.solarSystemId) INNER JOIN mapSolarSystems AS mssb ";
        query += "ON(msc.systemB = mssb.solarSystemId);";

        let mut statement = connection.prepare(query.as_str())?;
        let mut rows = statement.query([])?;
        let mut hmap: HashMap<String, MapLine> = HashMap::new();
        while let Some(row) = rows.next()? {
            let point1 = RawPoint::from([row.get::<usize, f32>(1)? as i64, row.get::<usize, f32>(2)? as i64, row.get::<usize, f32>(3)? as i64]);
            let point2 = RawPoint::from([row.get::<usize, f32>(4)? as i64, row.get::<usize, f32>(5)? as i64, row.get::<usize, f32>(6)? as i64]);
            let mut line = MapLine::new(point1,point2);
            line.id = Some(row.get::<usize, String>(0)?);
            let id = row.get::<usize, String>(0)?;
            hmap.entry(id).or_insert(line);
        }
        Ok(hmap)

    }
}
