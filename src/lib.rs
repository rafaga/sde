#![crate_name = "sde"]
//! Read Eve Online's SDE data from sqlite database
//!
//! Provides an abstraction layer over SDE data .
//! When the abstraction is used makes it fast to search
//! there are these advantages:
//!
//!
use crate::objects::{SdePoint, Universe, Region, SolarSystem, Constellation, Planet, Moon};
use egui_map::map::objects::{MapLine, MapPoint, RawPoint};
use objects::EveRegionArea;
use rusqlite::{params, vtab::array, Connection, Error, OpenFlags};
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;

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

        // Getting all regions in the main process because is not very intensive
        let regions = self.get_region(Vec::new())?;
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

        let constellations = self.get_constellation(parent_ids)?;

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

        let solar_systems =
            self.get_solarsystem( parent_ids)?;

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
        let connection = self.get_standart_connection()?;

        let mut hash_map: HashMap<usize, MapPoint> = HashMap::new();
        // centerX, centerY, centerZ,
        let mut query = String::from("SELECT SolarSystemId, projX, projY, projZ, SolarSystemName ");
        query += " FROM mapSolarSystems WHERE SolarSystemId BETWEEN ?1 AND ?2;";
        let mut statement = connection.prepare(query.as_str())?;
        let mut rows = statement.query(params![30000000, 30999999])?;
        let mut min_id = usize::MAX;
        while let Some(row) = rows.next()? {
            let id = row.get(0)?;
            if id < min_id {
                min_id = id;
            }
            let x = row.get::<usize, f32>(1)?;
            let y = row.get::<usize, f32>(2)?;
            let z = row.get::<usize, f32>(3)?;

            //we get the coordinate point and multiply with the adjust factor
            let mut coord = SdePoint::from([x as i64, y as i64, z as i64]);
            coord /= self.factor;
            if self.invert_coordinates {
                coord *= -1;
            }
            let mut point = MapPoint::new(id, coord.to_rawpoint());
            point.set_name(row.get::<usize, String>(4)?);
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

        let connection = self.get_standart_connection()?;

        let mut query = String::from("SELECT systemConnectionId, ");
        query += "systemA, systemB FROM mapSystemConnections;";

        let mut statement = connection.prepare(query.as_str())?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            // Optimization: to avoid printing twice the same line, we are just skipping coordinates
            // for SolarSystems that has an Id less than the current one printed. with the exception
            // of the lowest ID
            let id = row.get::<usize, String>(0)?;
            let system_a = row.get::<usize, usize>(1)?;
            let system_b = row.get::<usize, usize>(2)?;

            //we compare the current system with the first, if not the same then we add the coordinates to hashmap

            hash_map.entry(system_a).and_modify(|point| {
                point.connections.push(id.clone());
            });

            hash_map.entry(system_b).and_modify(|point| {
                point.connections.push(id);
            });
        }
        Ok(hash_map)
    }

    pub fn get_region_coordinates(&self) -> Result<Vec<EveRegionArea>, Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_scope!("get_region_coordinates");
        let connection = self.get_standart_connection()?;

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
            region.max = SdePoint::from([
                row.get::<usize, i64>(2)?,
                row.get::<usize, i64>(3)?,
                row.get::<usize, i64>(4)?,
            ]);
            region.min = SdePoint::from([
                row.get::<usize, i64>(5)?,
                row.get::<usize, i64>(6)?,
                row.get::<usize, i64>(7)?,
            ]);
            // we invert the coordinates and swap the min with the max
            if self.invert_coordinates {
                std::mem::swap(&mut region.max, &mut region.min);
                region.min *= -1;
                region.max *= -1;
            }
            areas.push(region);
        }
        Ok(areas)
    }

    pub fn get_system_id(&self, name: String) -> Result<Vec<(usize, String, usize, String)>, Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_scope!("get_system_id");
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

    pub fn get_system_coords(&self, id_node: usize) -> Result<Option<SdePoint>, Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_scope!("get_system_coords");
        let connection = self.get_standart_connection()?;

        let mut query = String::from("SELECT mss.ProjX, mss.ProjY, mss.ProjZ ");
        query += "FROM mapSolarSystems AS mss WHERE mss.SolarSystemId = ?1; ";

        let mut statement = connection.prepare(query.as_str())?;
        let system_like_name = id_node.to_string();
        let mut rows = statement.query(params![system_like_name])?;
        if let Some(row) = rows.next()? {
            let mut coord = SdePoint::from([
                row.get::<usize, f32>(0)?,
                row.get::<usize, f32>(1)?,
                row.get::<usize, f32>(2)?,
            ]);
            coord /= self.factor;
            if self.invert_coordinates {
                coord *= -1;
            }
            return Ok(Some(coord));
        }
        Ok(None)
    }

    pub fn get_connections(&self) -> Result<HashMap<String, MapLine>, Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_scope!("get_connections");

        let connection = self.get_standart_connection()?;

        let mut query = String::from("SELECT msc.systemConnectionId, ");
        query += "mssa.projX, mssa.projY, mssa.projZ, mssb.projX, mssb.projY, mssb.projZ ";
        query += "FROM mapSystemConnections AS msc INNER JOIN mapSolarSystems AS mssa ";
        query += "ON(msc.systemA = mssa.solarSystemId) INNER JOIN mapSolarSystems AS mssb ";
        query += "ON(msc.systemB = mssb.solarSystemId);";

        let mut statement = connection.prepare(query.as_str())?;
        let mut rows = statement.query([])?;
        let mut hmap: HashMap<String, MapLine> = HashMap::new();
        while let Some(row) = rows.next()? {
            let mut point1 = RawPoint::from([
                row.get::<usize, f32>(1)? as i64,
                row.get::<usize, f32>(3)? as i64,
            ]);
            let mut point2 = RawPoint::from([
                row.get::<usize, f32>(4)? as i64,
                row.get::<usize, f32>(6)? as i64,
            ]);
            point1 /= self.factor;
            point2 /= self.factor;
            if self.invert_coordinates {
                point1 *= -1;
                point2 *= -1;
            }
            let mut line = MapLine::new(point1, point2);
            line.id = Some(row.get::<usize, String>(0)?);
            let id = row.get::<usize, String>(0)?;
            hmap.entry(id).or_insert(line);
        }
        Ok(hmap)
    }

    pub fn get_abstract_systems(
        &self,
        regions: Vec<u32>,
    ) -> Result<HashMap<usize, MapPoint>, Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_scope!("get_abstract_systems");

        let connection = self.get_standart_connection()?;

        let mut query = String::from("SELECT mas.solarSystemId, ");
        query += "mas.x, mas.y, mas.regionId FROM mapAbstractSystems ";
        if !regions.is_empty() {
            query += " WHERE regionId IN rarray(?1);";
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
        let mut hash_map: HashMap<usize, MapPoint> = HashMap::new();
        while let Some(row) = rows.next()? {
            let point = MapPoint::new(
                row.get::<usize, usize>(0)?,
                RawPoint::new(row.get::<usize, f32>(1)?, row.get::<usize, f32>(2)?),
            );
            hash_map.insert(row.get::<usize, usize>(0)?, point);
        }
        Ok(hash_map)
    }

    pub fn get_abstract_system_connections(
        &self,
        mut hash_map: HashMap<usize, MapPoint>,
        regions: Vec<u32>,
    ) -> Result<HashMap<usize, MapPoint>, Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_scope!("get_abstract_system_connections");

        let connection = self.get_standart_connection()?;

        let mut query =
            String::from("SELECT mas.solarSystemId, mas.regionId, msc.systemConnectionId ");
        query += " FROM mapAbstractSystems AS mas INNER JOIN mapSystemConnections AS msc ";
        query += " ON(msc.systemA = mas.solarSystemId OR msc.systemB = mas.solarSystemId) ";
        if !regions.is_empty() {
            query += " WHERE mas.regionId IN rarray(?1);";
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
        while let Some(row) = rows.next()? {
            hash_map
                .entry(row.get::<usize, usize>(0)?)
                .and_modify(|map_point| {
                    if let Ok(hash) = row.get::<usize, String>(2) {
                        map_point.connections.push(hash);
                    }
                });
        }
        Ok(hash_map)
    }

    pub fn get_abstract_connections(
        &self,
        regions: Vec<u32>,
    ) -> Result<HashMap<String, MapLine>, Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_scope!("get_abstract_connections");

        let connection = self.get_standart_connection()?;

        let mut query = String::from("SELECT msc.systemConnectionId, ");
        query += "masa.x, masa.y, masb.x, masb.y ";
        query += "FROM mapSystemConnections AS msc INNER JOIN mapAbstractSystems AS masa ";
        query += "ON(msc.systemA = masa.solarSystemId) INNER JOIN mapAbstractSystems AS masb ";
        query += "ON(msc.systemB = masb.solarSystemId) ";
        if !regions.is_empty() {
            query += " WHERE masa.regionId IN rarray(?1) OR masb.regionId IN rarray(?2);";
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

        let mut hash_map: HashMap<String, MapLine> = HashMap::new();
        while let Some(row) = rows.next()? {
            let mut line = MapLine::new(
                RawPoint::new(row.get::<usize, f32>(1)?, row.get::<usize, f32>(2)?),
                RawPoint::new(row.get::<usize, f32>(3)?, row.get::<usize, f32>(4)?),
            );
            line.id = Some(row.get::<usize, String>(0)?);
            hash_map.entry(row.get::<usize, String>(0)?).or_insert(line);
        }

        Ok(hash_map)
    }

    pub fn get_regions(&self, regions: Vec<u32>,) -> Result<Vec<(u32,String)>, Error>  {
        #[cfg(feature = "puffin")]
        puffin::profile_scope!("get_regions");

        let connection = self.get_standart_connection()?;

        let mut query = String::from("SELECT regionName, regionId ");
        query += "FROM mapRegions ";
        if !regions.is_empty() {
            query += " WHERE regionId IN rarray(?1)";
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
        let mut result = vec![];
        while let Some(row) = rows.next()? {
            let value = (row.get::<usize, u32>(1)?,row.get::<usize, String>(0)?);
            result.push(value);
        }
        Ok(result)
    }

    fn get_standart_connection(&self) -> Result<Connection, Error> {
        let mut flags = OpenFlags::default();
        flags.set(OpenFlags::SQLITE_OPEN_NO_MUTEX, false);
        flags.set(OpenFlags::SQLITE_OPEN_FULL_MUTEX, true);
        let connection = Connection::open_with_flags(self.path, flags)?;

        // we add the carray module disguised as rarray in rusqlite
        array::load_module(&connection)?;
        Ok(connection)
    }

    fn get_region(
        &self,
        regions: Vec<u32>,
    ) -> Result<Vec<Region>, Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_scope!("get_region");

        let connection = self.get_standart_connection()?;
        
        let mut query = String::from("SELECT regionId, regionName FROM mapRegions");
        if !regions.is_empty() {
            query += " WHERE regionId IN rarray(?1)";
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
        let mut result = vec![];

        while let Some(row) = rows.next()? {
            let mut region = Region::new();
            region.id = row.get(0)?;
            region.name = row.get(1)?;
            result.push(region);
        }

        let query = "SELECT constellationId FROM mapConstellations WHERE regionId=?";
        
        for index in 0..result.len() {
            let mut statement = connection.prepare(query)?;
            let mut rows = statement.query([result[index].id])?;
            while let Some(row) = rows.next()? {
                result[index].constellations.push(row.get(0)?);
            }
        }
        Ok(result)
    }

    fn get_solarsystem(
        &self,
        constellation: Vec<u32>
    ) -> Result<Vec<SolarSystem>, Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_scope!("get_solarsystem");

        // preparing the connections that will be shared between threads
        let connection = self.get_standart_connection()?;
        let mut result = vec![];

        let mut query = String::from("SELECT mss.solarSystemId, mss.solarSystemName, mc.regionId, ");
        query += " mc.centerX, mc.centerY, mc.centerZ, mss.projX, mss.projY, mss.projZ, ";
        query += " mss.constellationId FROM mapSolarSystems AS mss ";
        query += " INNER JOIN mapConstellations AS mc ON(mss.constellationId = mc.constellationId)  ";
        if !constellation.is_empty() {
            query += " WHERE mss.constellationId IN rarray(?1);";
        }
        let mut statement = connection.prepare(query.as_str())?;

        let id_list = Rc::new(
            constellation
                .into_iter()
                .map(rusqlite::types::Value::from)
                .collect::<Vec<rusqlite::types::Value>>(),
        );

        let mut rows = statement.query(params![id_list])?;

        while let Some(row) = rows.next()?{
            let mut object = SolarSystem::new(self.factor);
            object.id = row.get(0)?;
            object.name = row.get(1)?;
            object.constellation = row.get(8)?;
            object.real_coords.x = row.get::<_, f64>(3)? as i64; //i64
            object.real_coords.y = row.get::<_, f64>(4)? as i64; //i64
            object.real_coords.z = row.get::<_, f64>(5)? as i64; //i64
            object.projected_coords.x = row.get::<_, f64>(6)? as i64; //i64
            object.projected_coords.y = row.get::<_, f64>(7)? as i64; //i64

            // Invert coordinates if needed
            if self.invert_coordinates {
                object.real_coords.x *= -1;
                object.real_coords.y *= -1;
                object.real_coords.z *= -1;
                object.projected_coords.x *= -1;
                object.projected_coords.y *= -1;
            }
            object.region = row.get(2)?;
            result.push(object);
        }
        let mut query = String::from(" SELECT msg.solarSystemId FROM mapSystemGates ");
        query += " AS msg WHERE msg.systemGateId ";
        query += " IN (SELECT destination FROM mapSystemGates AS msg ";
        query += " WHERE solarSystemId = ?1);";
        for index in 0..result.len() {
            let mut statement = connection.prepare(query.as_str())?;
            let mut rows = statement.query([result[index].id])?;
            while let Some(row) = rows.next()? {
                result[index].connections.push(row.get(0)?);
            }
        }
        Ok(result)
    }

    /// Function to get every Constellation or a Constellation based on an specific Region
    fn get_constellation(
        &self,
        regions: Vec<u32>,
    ) -> Result<Vec<Constellation>, Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_scope!("get_constellation");
        // preparing the connections that will be shared between threads
        let connection =  self.get_standart_connection()?;
        let mut result = vec![];

        let mut query = String::from("SELECT constellationId, constellationName, regionId ");
        query += "FROM mapConstellations ";
        if !regions.is_empty() {
            query += "WHERE regionId IN rarray(?1);";
        }

        let mut statement = connection.prepare(query.as_str())?;
        let id_list = Rc::new(
            regions
                .into_iter()
                .map(rusqlite::types::Value::from)
                .collect::<Vec<rusqlite::types::Value>>(),
        );
        let mut rows = statement.query(params![id_list])?;

        //while there are regions left to consume
        while let Some(row) = rows.next()? {
            let mut object = Constellation::new();
            object.id = row.get(0)?;
            object.name = row.get(1)?;
            object.region = row.get(2)?;
            result.push(object);
        }

        let query = "SELECT solarSystemId FROM mapSolarSystems WHERE constellationId = ?1";
        
        for index in 0..result.len() {
            let mut statement = connection.prepare(query)?;
            let mut rows = statement.query([result[index].id])?;
            while let Some(row) = rows.next()? {
                result[index].solar_systems.push(row.get(0).unwrap());
            }
        }

        Ok(result)
    }

    /// Function to get every Planet or all Planets for a specific Solar System
    pub fn get_planet(
        &self,
        solar_systems: Vec<u32>,
    ) -> Result<Vec<Planet>, Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_scope!("get_planet");
        // preparing the connections that will be shared between threads
        let connection =  self.get_standart_connection()?;
        let mut result = vec![];

        let mut query = String::from("SELECT planetId, planetaryIndex, solarSystemId");
        query += " FROM mapPlanets";
        if !solar_systems.is_empty() {
            query += " WHERE solarSystemId IN rarray(?1)";
        }

        let mut statement = connection.prepare(query.as_str())?;
        let id_list = Rc::new(
            solar_systems
                .into_iter()
                .map(rusqlite::types::Value::from)
                .collect::<Vec<rusqlite::types::Value>>(),
        );
        let mut rows = statement.query(params![id_list])?;

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
    pub fn get_moon(
        &self,
        planets: Vec<u32>,
    ) -> Result<Vec<Moon>, Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_scope!("get_moon");

        // preparing the connections that will be shared between threads
        let connection =  self.get_standart_connection()?;
        let mut result = vec![];

        let mut query = String::from(
            "SELECT moonId, moonIndex, solarSystemId, planetId ");
        query += "FROM mapMoons ";
       
        if !planets.is_empty() {
            query += " WHERE planetId=?";
        };

        let mut statement = connection.prepare(query.as_str()).unwrap();
        let id_list = Rc::new(
            planets
                .into_iter()
                .map(rusqlite::types::Value::from)
                .collect::<Vec<rusqlite::types::Value>>(),
        );
        let mut rows = statement.query(params![id_list])?;
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
