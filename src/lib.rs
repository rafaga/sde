#![crate_name = "sde"]
//! Read Eve Online's SDE data from sqlite database
//!
//! Provides an abstraction layer over SDE data .
//! When the abstraction is used makes it fast to search
//! there are these advantages:
//!
//!
use crate::objects::Universe;
use objects::EveRegionArea;
use rusqlite::{Connection, Error, OpenFlags};
use std::path::Path;
use egui_map::map::objects::{MapPoint,MapLine};
use std::collections::HashMap;

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
    pub factor: i64
}


impl<'a> SdeManager<'a> {
    /// Creates a new SdeManager using a path to build the connection
    pub fn new(path: &Path, factor: i64) -> SdeManager {
        SdeManager { 
            path,
            universe: Universe::new(factor),
            factor: factor, // 10000000000000
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
            self.universe.dicts
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
            self.universe.dicts
                .constellation_names
                .entry(constel.name.to_lowercase().clone())
                .or_insert(constel.id);
            self.universe.constellations.entry(constel.id).or_insert(constel);
        }

        let parent_ids = match parent_ids.len() {
            x if x > 0 => Some(parent_ids),
            _ => None,
        };

        let mut flags = OpenFlags::default();
        flags.set(OpenFlags::SQLITE_OPEN_NO_MUTEX, false);
        flags.set(OpenFlags::SQLITE_OPEN_FULL_MUTEX, true);
        let connection = Connection::open_with_flags(self.path, flags)?;
        let solar_systems = self.universe.get_solarsystem(connection, parent_ids)?;

        let mut parent_ids = vec![];
        for system in solar_systems {
            #[cfg(feature = "puffin")]
            puffin::profile_scope!("hashmap_add_planetary_systems");
            parent_ids.push(system.id);
            self.universe.dicts
                .constellation_names
                .entry(system.name.to_lowercase().clone())
                .or_insert(system.id);
            self.universe.solar_systems.entry(system.id).or_insert(system);
        }
        Ok(true)
    }

    /// Function to get all the K-Space solar systems coordinates from the SDE including data to build a map
    /// and search for basic stuff
    pub fn get_systempoints(&self,dimentions: u8) -> Result<HashMap<usize,MapPoint>, Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_scope!("get_systempoints");

        let mut hash_map:HashMap<usize,MapPoint> = HashMap::new();
        let mut flags = OpenFlags::default();
        flags.set(OpenFlags::SQLITE_OPEN_NO_MUTEX, false);
        flags.set(OpenFlags::SQLITE_OPEN_FULL_MUTEX, true);
        let connection = Connection::open_with_flags(self.path, flags)?;

        let mut query = String::from("SELECT SolarSystemId, centerX, centerY, centerZ, projX, projY, SolarSystemName ");
        query += " FROM mapSolarSystems WHERE SolarSystemId BETWEEN 30000000 AND 30999999;";
        let mut statement = connection.prepare(query.as_str())?;
        let mut rows = statement.query([])?;
        let mut min_id = usize::MAX;
        while let Some(row) = rows.next()? {
            #[cfg(feature = "puffin")]
            puffin::profile_scope!("getting system points");

            let id:usize = row.get(0)?;
            if id < min_id {
                min_id = id;
            }
            let mut _coords = Vec::new();
            if dimentions == 2 {
                _coords = vec![row.get(4)?,row.get(5)?];
            } else {
                _coords = vec![row.get(1)?,row.get(2)?,row.get(3)?];
            }
            _coords[0] = _coords[0] / self.factor as f64;
            _coords[1] = _coords[1] / self.factor as f64;
            if dimentions == 3 {
                _coords[2] = _coords[2] / self.factor as f64;
            }
            let mut point = MapPoint::new(id,_coords);
            point.name = row.get(6)?;
            hash_map.insert(id, point);
        }
        Ok(hash_map)
    }

    pub fn get_regional_connections(&self) -> Result<Vec<MapLine>,Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_scope!("get_regional_connections");

        let mut flags = OpenFlags::default();
        flags.set(OpenFlags::SQLITE_OPEN_NO_MUTEX, false);
        flags.set(OpenFlags::SQLITE_OPEN_FULL_MUTEX, true);
        let connection = Connection::open_with_flags(self.path, flags)?;

        let mut query = String::from("SELECT msga.solarSystemId AS origin, mpsa.projX as originX, mpsa.projY as originY, mpsb.SolarSystemId AS destination,");
        query += "mpsb.projX as destinationX, mpsb.projY as destinationY ";
        query += "FROM mapSystemGates AS msga ";
        query += "INNER JOIN mapSystemGates AS msgb ON (msgb.systemGateId = msga.destination) ";
        query += "INNER JOIN mapSolarSystems AS mpsa ON (mpsa.solarSystemId = msga.solarSystemId) ";
        query += "INNER JOIN mapSolarSystems AS mpsb ON (mpsb.solarSystemId = msgb.solarSystemId) ";
        query += "INNER JOIN mapConstellations AS mca ON (mca.constellationId = mpsa.constellationId) ";
        query += "INNER JOIN mapConstellations AS mcb ON (mcb.constellationId = mpsb.constellationId) ";
        query += "WHERE mca.regionId <> mcb.regionId AND mpsa.solarSystemId < mpsb.solarSystemId ";

        let mut statement = connection.prepare(query.as_str())?;
        let mut rows = statement.query([])?;
        let mut vec_lines = Vec::new();
        let ffactor = self.factor as f32;
        while let Some(row) = rows.next()? {
            let cords:[f32;4] = [row.get(1)?, row.get(2)?, row.get(4)?, row.get(5)?];
            vec_lines.push(MapLine::new(cords[0] / ffactor,cords[1] / ffactor,cords[2] / ffactor,cords[3] / ffactor));
        }
        Ok(vec_lines)
    } 

    pub fn get_connections(&self, mut hash_map: HashMap<usize,MapPoint>, dimentions: u8) -> Result<HashMap<usize,MapPoint>,Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_scope!("get_connections");

        let mut flags = OpenFlags::default();
        flags.set(OpenFlags::SQLITE_OPEN_NO_MUTEX, false);
        flags.set(OpenFlags::SQLITE_OPEN_FULL_MUTEX, true);
        let connection = Connection::open_with_flags(self.path, flags)?;
        
        let mut query = String::from("SELECT msga.solarSystemId AS origin, mps.SolarSystemId AS destination, mps.centerX, ");
        query += "mps.centerY, mps.centerZ, mps.projX, mps.projY FROM mapSystemGates AS msga ";
        query += "INNER JOIN mapSystemGates AS msgb ON (msgb.systemGateId = msga.destination) ";
        query += "INNER JOIN mapSolarSystems AS mps ON (mps.solarSystemId = msgb.solarSystemId)";
        query += "ORDER BY 1 ASC";

        let mut statement = connection.prepare(query.as_str())?;
        let mut rows = statement.query([])?;
        let mut id:(usize,usize) = (0,0);
        let mut vec_coords: Vec<[f64; 3]> = Vec::new();
        let mut mapped:HashMap<usize,usize> = HashMap::new();
        while let Some(row) = rows.next()? {

            // Optimization: to avoid printing twice the same line, we are just skipping coordinates
            // for SolarSystems that has an Id less than the current one printed. with the exception
            // of the lowest ID
            let origin = row.get(0)?;
            let destination = row.get::<usize,usize>(1)?;

            // we store the first system
            if id.1 == 0 {
                mapped.entry(id.0).or_insert(1);
                id.0 = origin;
            } 

            //we compare the current system with the first, if not the same then we add the coordinates to hashmap
            if id.1 != origin {
                hash_map.entry(id.1).and_modify(|point| { point.lines=vec_coords.clone() });
                vec_coords.clear();
            }

            // we add the current origin system
            id.1 = origin;
            // if destination point is already mapped and not the fist node then we skip it
            if mapped.contains_key(&destination) && origin != id.0 {
                continue;
            }

            // initialize a coordinate in 
            let mut coords:[f64; 3]= [0.0,0.0,0.0];

            //we get the coordinate point and multiply with the adjust factor
            coords[0] = row.get(5)?;
            coords[1] = row.get(6)?;
            coords[0] = coords[0] / self.factor as f64;
            coords[1] = coords[1] / self.factor as f64;

            // if we had a third dimesion we add the Z axis coordinate 
            if dimentions == 3 {
                coords = [row.get(2)?,row.get(3)?,row.get(4)?];
                coords[2] = coords[2] / self.factor as f64;
            }

            // we add the coordinates to the vector
            vec_coords.push(coords);
        }
        // we add the last point to the hashmap
        if id.1 != 0 {
            hash_map.entry(id.1).and_modify(|point| { point.lines=vec_coords });
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
        let mut query = "SELECT reg.regionId, reg.regionName, MAX(reg.max_x) AS region_max_x,".to_string();
        query += "MAX(reg.max_y) AS region_max_y, MIN(reg.min_x) AS region_min_x, ";
        query += "MIN(reg.min_y) AS region_min_y FROM (SELECT mr.regionId, mr.regionName, ";
        query += "mc.constellationId, MAX(mss.projX) AS max_x, MAX(mss.projY) AS max_y, ";
        query += "MIN(mss.projX) AS min_x, MIN(mss.projY) AS min_y FROM mapRegions AS mr ";
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
            region.max.x = row.get(2)?;
            region.max.y = row.get(3)?;
            region.min.x = row.get(4)?;
            region.min.y = row.get(5)?;
            areas.push(region); 
        }
        Ok(areas)
    }

    pub fn get_system_id(&self,name: usize) -> usize {
        0
    }
}
