#![warn(missing_docs)]
#![crate_name = "sde"]
//! Read Eve Online's SDE data from sqlite database
//!
//! Provides an abstraction layer over SDE data .
//! When the abstraction is used makes it fast to search
//! there are these advantages:
//!
//!
use crate::objects::{Universe,SystemPoint};
use rusqlite::{Connection, Error, OpenFlags};
use std::{path::Path};


/// Module that has Data object abstractions to fill with the database data.
pub mod objects;

/// Module that contains some hardcoded values useful to the crate
pub mod consts {

    /// Maximum number of threads to invoke in a multithread routines
    pub const MAX_THREADS: i8 = 8;
}


/// Manages the process of reading SDE data and putting into different data structures
/// for easy in-memory access.
pub struct SdeManager<'a> {
    /// The path to the SDE database
    pub path: &'a Path,
    /// This stores the current state of SDEManager for async porpourses
    pub state: State,
    /// The universe Object that contains all the data
    pub universe: Universe
}

// List of states our `async` block can be in
/// States in where SdeManager could be in 
pub enum State {
    /// SDEManager is not doing something and has not yet finished
    Awaiting,
    /// SDEManager has done its task
    Done,
}

impl<'a> SdeManager<'a> {
    /// Creates a new SdeManager using a path to build the connection
    pub fn new(path: &Path) -> SdeManager {
        SdeManager { 
            path,
            state: State::Awaiting,
            universe: Universe::new()
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
        self.state = State::Awaiting;
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
            parent_ids.push(system.id);
            self.universe.dicts
                .constellation_names
                .entry(system.name.to_lowercase().clone())
                .or_insert(system.id);
            self.universe.solar_systems.entry(system.id).or_insert(system);
        }
        self.state = State::Done;
        Ok(true)
    }

    /* 
    /// Function to get all the K-Space 3D coordinates from the SDE
    pub fn get_3dpoints(self) -> Result<Vec<Point3D>, Error> {
        let mut flags = OpenFlags::default();
        flags.set(OpenFlags::SQLITE_OPEN_NO_MUTEX, false);
        flags.set(OpenFlags::SQLITE_OPEN_FULL_MUTEX, true);
        let connection = Connection::open_with_flags(self.path, flags)?;
        let mut query = String::from("SELECT SolarSystemId, centerX, centerY, centerZ ");
        query += " FROM mapSolarSystems WHERE SolarSystemId BETWEEN 30000000 AND 30999999;";
        let mut statement = connection.prepare(query.as_str())?;
        let mut rows = statement.query([])?;
        let mut pointk = Vec::new();
        while let Some(row) = rows.next()? {
            let x = row.get(1)?;
            let y = row.get(2)?;
            let z = row.get(3)?;
            let id = row.get(0)?;
            let point = Point3D::new(id, [x, y, z]);
            pointk.push(point);
        }
        Ok(pointk)
    }

    /// Function to get all the K-Space 2D coordinates from the SDE
    pub fn get_2dpoints(self) -> Result<Vec<Point2D>, Error> {
        let mut flags = OpenFlags::default();
        flags.set(OpenFlags::SQLITE_OPEN_NO_MUTEX, false);
        flags.set(OpenFlags::SQLITE_OPEN_FULL_MUTEX, true);
        let connection = Connection::open_with_flags(self.path, flags)?;
        let mut query = String::from("SELECT SolarSystemId, projX, projY ");
        query += " FROM mapSolarSystems WHERE SolarSystemId BETWEEN 30000000 AND 30999999;";
        let mut statement = connection.prepare(query.as_str())?;
        let mut rows = statement.query([])?;
        let mut pointk = Vec::new();
        while let Some(row) = rows.next()? {
            let x = row.get(1)?;
            let y = row.get(2)?;
            let id = row.get(0)?;
            let point = Point2D::new(id, [x, y]);
            pointk.push(point);
        }
        Ok(pointk)
    }*/

    /// Function to get all the K-Space solar systems coordinates from the SDE including data to build a map
    /// and serach for basic stuff
    pub fn get_systempoints(self,dimentions: u8) -> Result<Vec<SystemPoint>, Error> {
        let mut flags = OpenFlags::default();
        flags.set(OpenFlags::SQLITE_OPEN_NO_MUTEX, false);
        flags.set(OpenFlags::SQLITE_OPEN_FULL_MUTEX, true);
        let connection = Connection::open_with_flags(self.path, flags)?;
        let mut query = String::from("SELECT SolarSystemId, centerX, centerY, centerZ, projX, projY ");
        query += " FROM mapSolarSystems WHERE SolarSystemId BETWEEN 30000000 AND 30999999;";
        let mut statement = connection.prepare(query.as_str())?;
        let mut rows = statement.query([])?;
        let mut pointk = Vec::new();
        while let Some(row) = rows.next()? {
            let mut _coords = Vec::new();
            if dimentions == 2 {
                _coords = vec![row.get(4)?,row.get(5)?];
            } else {
                _coords = vec![row.get(1)?,row.get(2)?,row.get(3)?];
            }
            let id = row.get(0)?;
            let point = SystemPoint::new(id,_coords);
            pointk.push(point);
        }
        query = "SELECT mps.centerX, mps.centerY, mps.centerZ, mps.projX, mps.projY ".to_string();
        query += "FROM mapSolarSystems AS mps INNER JOIN mapSystemGates AS msg ";
        query += "ON (mps.SolarSystemId = msg.SolarSystemId) WHERE systemGateId IN ";
        query += "(SELECT msga.systemGateId FROM mapSystemGates AS msga INNER JOIN mapSystemGates AS msgb ";
        query += " ON (msga.systemGateId = msgb.destination) WHERE msgb.SolarSystemId=?)";
        for point in &mut pointk{
            let mut statement = connection.prepare(query.as_str())?;
            let mut rows = statement.query([&point.id])?;
            while let Some(row) = rows.next()? {
                let mut _coords= [row.get(3)?,row.get(4)?,0.0];
                if dimentions == 3 {
                    _coords = [row.get(0)?,row.get(1)?,row.get(2)?];
                }
                point.lines.push(_coords);

            }
        }
        Ok(pointk)
    }

}
