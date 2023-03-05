#![warn(missing_docs)]
#![crate_name = "sde"]
//! Read Eve Online's SDE data from sqlite database
//!
//! Provides an abstraction layer over SDE data .
//! When the abstraction is used makes it fast to search
//! there are these advantages:
//!
//!
use crate::objects::Universe;
use rusqlite::{Connection, Error, OpenFlags};
use std::path::Path;
use futures::{task::Poll,future::Future,task::Context};
use std::pin::Pin;


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

    /// Asynbc Call to get Universe function
    pub async fn get_async_universe(&mut self) -> Result<bool, Error> {
        self.get_universe()
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

    /// Function that extracts points from database and insert into Universe struct.
    pub fn get_points(mut self) -> Result<bool, Error> {
        self.state = State::Awaiting;
        if let Some(_) = self.universe.points {
            return Ok(false);
        }
        let mut flags = OpenFlags::default();
        flags.set(OpenFlags::SQLITE_OPEN_NO_MUTEX, false);
        flags.set(OpenFlags::SQLITE_OPEN_FULL_MUTEX, true);
        let connection = Connection::open_with_flags(self.path, flags)?;
        let vec_points = Universe::get_points(connection)?;
        self.universe.points = Some(vec_points);
        self.state = State::Done;
        Ok(true)
    }
}

impl<'a> Future for SdeManager<'a>{

    type Output = Result<bool, Error>;
    
    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> { 
        match self.state {
            State::Awaiting => return  Poll::Pending,
            State::Done => return Poll::Ready(Ok(true)),
        }
    }
}
