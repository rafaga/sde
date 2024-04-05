use super::consts;
use egui_map::map::objects::RawPoint;
use rusqlite::Error as RusqliteError;
use std::collections::HashMap;
use std::io::{Error as GenericError, ErrorKind};
use std::ops::{Add, Div, DivAssign, Mul, MulAssign, Sub};
use std::sync::{Arc, Mutex};
use std::{
    convert::{From, TryInto},
    thread, vec,
};

#[derive(Hash, PartialEq, Eq, Clone)]
pub struct EveRegionArea {
    pub region_id: u32,
    pub name: String,
    pub min: SdePoint,
    pub max: SdePoint,
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
            min: SdePoint::default(),
            max: SdePoint::default(),
        }
    }
}

#[derive(Hash, PartialEq, Eq, Clone)]
pub struct SdeLine {
    points: [SdePoint; 2],
}

impl SdeLine {
    pub fn new(a: SdePoint, b: SdePoint) -> Self {
        Self { points: [a, b] }
    }

    pub fn distance(self) -> f32 {
        let x = self.points[0].x - self.points[1].x;
        let y = self.points[0].y - self.points[1].y;
        let z = self.points[0].z - self.points[1].z;
        let value = (x.pow(2) + y.pow(2) + z.pow(2)) as f32;
        value.sqrt()
    }

    pub fn midpoint(self) -> SdePoint {
        let x = (self.points[0].x + self.points[1].x) / 2;
        let y = (self.points[0].y + self.points[1].y) / 2;
        let z = (self.points[0].z + self.points[1].z) / 2;
        SdePoint::new(x, y, z)
    }
}

#[derive(Hash, PartialEq, Eq, Clone)]
// This can by any object or point with its associated metadata
/// Struct that contains coordinates to help calculate nearest point in space
/// 3d point coordinates that it is used in:
///
/// - SolarSystems
pub struct SdePoint {
    /// X coorddinate
    pub x: i64,
    /// Y coordinate
    pub y: i64,
    /// Z coordinate
    pub z: i64,
}

impl SdePoint {
    /// Creates a new Coordinates struct. ALl the coordinates are initialized.
    pub fn new(x: i64, y: i64, z: i64) -> Self {
        SdePoint { x, y, z }
    }

    pub fn to_rawpoint(self) -> RawPoint {
        RawPoint::new(self.x as f32, self.z as f32)
    }
}

impl Default for SdePoint {
    fn default() -> Self {
        Self::new(0, 0, 0)
    }
}

impl From<[i64; 3]> for SdePoint {
    fn from(value: [i64; 3]) -> Self {
        Self {
            x: value[0],
            y: value[1],
            z: value[2],
        }
    }
}

impl From<SdePoint> for [i64; 3] {
    fn from(val: SdePoint) -> Self {
        [val.x, val.y, val.z]
    }
}

impl From<SdePoint> for [f64; 3] {
    fn from(val: SdePoint) -> Self {
        [val.x as f64, val.y as f64, val.z as f64]
    }
}

impl TryInto<[f32; 2]> for SdePoint {
    type Error = GenericError;

    fn try_into(self) -> Result<[f32; 2], <Self as TryInto<[f32; 2]>>::Error> {
        if self.x == 0 {
            Ok([self.y as f32, self.z as f32])
        } else if self.y == 0 {
            Ok([self.x as f32, self.z as f32])
        } else if self.z == 0 {
            Ok([self.x as f32, self.y as f32])
        } else {
            Err(GenericError::new(ErrorKind::NotFound,"projection pivot value not found, it is not possible to determine wich values to return."))
        }
    }
}

impl TryInto<[f32; 3]> for SdePoint {
    type Error = GenericError;

    fn try_into(self) -> Result<[f32; 3], <Self as TryInto<[f32; 3]>>::Error> {
        if self.x > f32::MAX as i64
            || self.x < f32::MIN as i64
            || self.y > f32::MAX as i64
            || self.y < f32::MIN as i64
            || self.z > f32::MAX as i64
            || self.z < f32::MIN as i64
        {
            return Err(GenericError::new(ErrorKind::InvalidData, "Value Overflow"));
        }
        Ok([self.x as f32, self.y as f32, self.z as f32])
    }
}

impl TryInto<[i64; 2]> for SdePoint {
    type Error = GenericError;

    fn try_into(self) -> Result<[i64; 2], <Self as TryInto<[i64; 2]>>::Error> {
        if self.x > f32::MAX as i64
            || self.x < f32::MIN as i64
            || self.y > f32::MAX as i64
            || self.y < f32::MIN as i64
            || self.z > f32::MAX as i64
            || self.z < f32::MIN as i64
        {
            return Err(GenericError::new(ErrorKind::InvalidData, "Value Overflow"));
        }
        if self.x == 0 {
            Ok([self.y, self.z])
        } else if self.y == 0 {
            Ok([self.x, self.z])
        } else if self.z == 0 {
            Ok([self.x, self.y])
        } else {
            Err(GenericError::new(ErrorKind::NotFound,"projection pivot value not found, it is not possible to determine wich values to return."))
        }
    }
}

impl From<[f32; 3]> for SdePoint {
    fn from(value: [f32; 3]) -> Self {
        Self {
            x: value[0].round() as i64,
            y: value[1].round() as i64,
            z: value[2].round() as i64,
        }
    }
}

impl DivAssign<isize> for SdePoint {
    fn div_assign(&mut self, rhs: isize) {
        self.x = self.x / rhs as i64;
        self.y = self.y / rhs as i64;
        self.z = self.z / rhs as i64;
    }
}

impl DivAssign<u64> for SdePoint {
    fn div_assign(&mut self, rhs: u64) {
        self.x = self.x / rhs as i64;
        self.y = self.y / rhs as i64;
        self.z = self.z / rhs as i64;
    }
}

impl DivAssign<i32> for SdePoint {
    fn div_assign(&mut self, rhs: i32) {
        self.x = self.x / rhs as i64;
        self.y = self.y / rhs as i64;
        self.z = self.z / rhs as i64;
    }
}

impl DivAssign<f32> for SdePoint {
    fn div_assign(&mut self, rhs: f32) {
        self.x = self.x / rhs.round() as i64;
        self.y = self.y / rhs.round() as i64;
        self.z = self.z / rhs.round() as i64;
    }
}

impl MulAssign<isize> for SdePoint {
    fn mul_assign(&mut self, rhs: isize) {
        self.x = self.x * rhs as i64;
        self.y = self.y * rhs as i64;
        self.z = self.z * rhs as i64;
    }
}

impl MulAssign<u64> for SdePoint {
    fn mul_assign(&mut self, rhs: u64) {
        self.x = self.x * rhs as i64;
        self.y = self.y * rhs as i64;
        self.z = self.z * rhs as i64;
    }
}

impl MulAssign<i32> for SdePoint {
    fn mul_assign(&mut self, rhs: i32) {
        self.x = self.x * rhs as i64;
        self.y = self.y * rhs as i64;
        self.z = self.z * rhs as i64;
    }
}

impl MulAssign<f32> for SdePoint {
    fn mul_assign(&mut self, rhs: f32) {
        self.x = self.x * rhs.round() as i64;
        self.y = self.y * rhs.round() as i64;
        self.z = self.z * rhs.round() as i64;
    }
}

impl Mul<isize> for SdePoint {
    type Output = Self;
    fn mul(self, rhs: isize) -> Self::Output {
        Self {
            x: self.x * rhs as i64,
            y: self.y * rhs as i64,
            z: self.z * rhs as i64,
        }
    }
}

impl Div<isize> for SdePoint {
    type Output = Self;
    fn div(self, rhs: isize) -> Self::Output {
        Self {
            x: self.x / rhs as i64,
            y: self.y / rhs as i64,
            z: self.z / rhs as i64,
        }
    }
}

impl Add<SdePoint> for SdePoint {
    type Output = SdePoint;
    fn add(self, rhs: SdePoint) -> Self::Output {
        SdePoint {
            x: self.x + rhs.x,
            y: self.y + rhs.y,
            z: self.z + rhs.z,
        }
    }
}

impl Sub<SdePoint> for SdePoint {
    type Output = SdePoint;
    fn sub(self, rhs: SdePoint) -> Self::Output {
        SdePoint {
            x: self.x - rhs.x,
            y: self.y - rhs.y,
            z: self.z - rhs.z,
        }
    }
}

impl Add<&SdePoint> for SdePoint {
    type Output = SdePoint;
    fn add(self, rhs: &SdePoint) -> Self::Output {
        SdePoint {
            x: self.x + rhs.x,
            y: self.y + rhs.y,
            z: self.z + rhs.z,
        }
    }
}

impl Sub<&SdePoint> for SdePoint {
    type Output = SdePoint;
    fn sub(self, rhs: &SdePoint) -> Self::Output {
        SdePoint {
            x: self.x - rhs.x,
            y: self.y - rhs.y,
            z: self.z - rhs.z,
        }
    }
}

/// Abstraction for a Planet Moons. It store data relevant to this entity
#[derive(Hash, PartialEq, Eq, Clone)]
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
#[derive(Hash, PartialEq, Eq, Clone)]
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
#[derive(Hash, PartialEq, Eq, Clone)]
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
    pub real_coords: SdePoint,
    /// Solar System 2D Coordinates with the propourse of representing the system in abstraction map.
    pub projected_coords: SdePoint,
    /// The factor that we need to adjust the coordinates
    pub factor: u64,
}

impl SolarSystem {
    /// Creates a new Solar System Strcut. ALl the values are initialized. Needs to be filled
    pub fn new(factor: u64) -> Self {
        SolarSystem {
            id: 0,
            name: String::new(),
            region: 0,
            constellation: 0,
            planets: Vec::new(),
            connections: Vec::new(),
            real_coords: SdePoint::default(),
            projected_coords: SdePoint::default(),
            factor,
        }
    }

    /// this function that correct the original 2d coordinates using the correction factor
    pub fn coord2d_to_f64(self) -> [f64; 2] {
        [
            (self.projected_coords.x / self.factor as i64) as f64,
            (self.real_coords.y / self.factor as i64) as f64,
        ]
    }

    /// this function that correct the original 3d coordinates using the correction factor
    pub fn coord3d_to_f64(self) -> [f64; 3] {
        [
            (self.projected_coords.x / self.factor as i64) as f64,
            (self.real_coords.y / self.factor as i64) as f64,
            (self.real_coords.z / self.factor as i64) as f64,
        ]
    }
}

impl Default for SolarSystem {
    fn default() -> Self {
        Self::new(1)
    }
}

/// Abstraction for a Constellation. It store data relevant to this entity
#[derive(Hash, PartialEq, Eq, Clone)]
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
    pub projected_coords: SdePoint,
}

impl Constellation {
    /// Creates a new Constellation Strcut. ALl the values are initialized. Needs to be filled
    pub fn new() -> Self {
        Constellation {
            id: 0,
            name: String::new(),
            region: 0,
            solar_systems: Vec::new(),
            projected_coords: SdePoint::default(),
        }
    }
}

impl Default for Constellation {
    fn default() -> Self {
        Self::new()
    }
}

/// Abstraction for a Region. It store data relevant to this entity
#[derive(Hash, PartialEq, Eq, Clone)]
pub struct Region {
    /// Region Identifier
    pub id: u32,
    /// Region Name
    pub name: String,
    /// Vector with Region's Constellationm Identifiers
    pub constellations: Vec<u32>,
    /// Region 2D Coordinates with the propourse of representing the system in abstraction map.
    pub projected_coords: SdePoint,
}

impl Region {
    /// Creates a new Region Strcut. ALl the values are initialized. Needs to be filled
    pub fn new() -> Self {
        Region {
            id: 0,
            name: String::new(),
            constellations: Vec::new(),
            projected_coords: SdePoint::default(),
        }
    }
}

impl Default for Region {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(PartialEq, Eq, Clone)]
/// Struct that contains Dictionary to search regions, constellations and solarsystems by name
pub struct Dictionaries {
    /// Solar system dictionary
    pub system_names: HashMap<String, u32>,
    /// Constellations dictionary
    pub constellation_names: HashMap<String, u32>,
    /// Region dictionary
    pub region_names: HashMap<String, u32>,
}

impl Dictionaries {
    /// Creates a new Dictionaries Strcut. ALl the values are initialized. Needs to be filled
    pub fn new() -> Dictionaries {
        Dictionaries {
            system_names: HashMap::new(),
            constellation_names: HashMap::new(),
            region_names: HashMap::new(),
        }
    }
}

impl Default for Dictionaries {
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
    /// Dictionaries struct
    pub dicts: Dictionaries,
    /// Factor used to correct coordinates
    pub factor: u64,
    /// List of system connections
    pub connections: HashMap<String, SdeLine>,
}

impl Universe {
    /// Creates a new Universe Strcut. ALl the values are initialized. Needs to be filled
    pub fn new(factor: u64) -> Universe {
        Universe {
            regions: HashMap::new(),
            constellations: HashMap::new(),
            solar_systems: HashMap::new(),
            planets: HashMap::new(),
            moons: HashMap::new(),
            dicts: Dictionaries::new(),
            factor,
            connections: HashMap::new(),
        }
    }

    /// Function to get every region available in SDE database
    pub fn get_region(
        &self,
        connection: &rusqlite::Connection,
        regions: Option<Vec<u32>>,
    ) -> Result<Vec<Region>, RusqliteError> {
        #[cfg(feature = "puffin")]
        puffin::profile_scope!("get_region");

        let mut query = String::from("SELECT regionId, regionName FROM mapRegions");
        let mut temp_regions = Vec::new();
        let param_values: Vec<u32> = Vec::new();
        let mut regions_result = Vec::new();
        let mut vec_params = Vec::new();

        if let Some(temp_vec) = regions {
            query += " WHERE regionId=?;";
            vec_params = temp_vec;
        }
        loop {
            let mut statement = connection.prepare(query.as_str())?;
            let mut rows;
            if !vec_params.is_empty() {
                rows = statement.query([vec_params.pop().unwrap()])?;
            } else {
                rows = statement.query([])?;
            }
            while let Some(row) = rows.next()? {
                let mut region = Region::new();
                region.id = row.get(0)?;
                region.name = row.get(1)?;
                temp_regions.push(region);
            }
            let query = "SELECT constellationId FROM mapConstellations WHERE regionId=?";

            loop {
                let mut region = temp_regions.pop().unwrap();
                let mut statement = connection.prepare(query)?;
                let mut rows = statement.query([region.id])?;
                while let Some(row) = rows.next()? {
                    region.constellations.push(row.get(0)?);
                }
                regions_result.push(region);
                if temp_regions.is_empty() {
                    break;
                }
            }
            if param_values.is_empty() {
                break;
            }
        }
        Ok(regions_result)
    }

    /// Function to get every Solarsystem or a Solar systems for a specific Constellation
    pub fn get_solarsystem(
        &self,
        connection: rusqlite::Connection,
        constellation: Option<Vec<u32>>,
        invert_coordinates: bool,
    ) -> Result<Vec<SolarSystem>, RusqliteError> {
        #[cfg(feature = "puffin")]
        puffin::profile_scope!("get_solarsystem");

        // preparing the connections that will be shared between threads
        let kconn = Arc::new(Mutex::new(connection));
        let mut handles = vec![];

        //Preparing the Mutexed Vector to get all constellations
        let vec_objects = Arc::new(Mutex::new(Vec::new()));

        // Preparing a Mutexed Vector with region ids data
        let vec_parent_ids = match constellation {
            Some(temp_vec) => Arc::new(Mutex::new(temp_vec)),
            None => Arc::new(Mutex::new(vec![])),
        };
        for _x in (0..consts::MAX_THREADS).collect::<std::vec::Vec<i8>>() {
            // cloning Objects to invoke a thread
            let sh_objects = Arc::clone(&vec_objects);
            let sh_parent_ids = Arc::clone(&vec_parent_ids);
            let sh_conn = Arc::clone(&kconn);
            let temp_factor = self.factor;
            // invoke a thread

            let handle = thread::spawn(move || {
                let thread_connection = &sh_conn.lock().unwrap();
                let mut query =
                    String::from("SELECT mss.solarSystemId, mss.solarSystemName, mc.regionId, ");
                query += " mc.centerX, mc.centerY, mc.centerZ, mss.projX, mss.projY, mss.projZ, ";
                query += " mss.constellationId FROM mapSolarSystems AS mss ";
                query += " INNER JOIN mapConstellations AS mc ON(mss.constellationId = mc.constellationId)  ";
                let vec_parent_ids = &mut sh_parent_ids.lock().unwrap();
                if vec_parent_ids.len() > 0 {
                    query += " WHERE mss.constellationId=? ";
                };
                let mut temp_vec = Vec::new();
                loop {
                    let mut statement = thread_connection.prepare(query.as_str()).unwrap();
                    let mut rows;
                    if vec_parent_ids.len() > 0 {
                        rows = statement.query([vec_parent_ids.pop().unwrap()]).unwrap();
                    } else {
                        rows = statement.query([]).unwrap();
                    }
                    //while there are constellations left to consume
                    while let Some(row) = rows.next().unwrap() {
                        let mut object = SolarSystem::new(temp_factor);
                        object.id = row.get(0).unwrap();
                        object.name = row.get(1).unwrap();
                        object.constellation = row.get(8).unwrap();
                        object.real_coords.x = row.get::<_, f64>(3).unwrap() as i64; //i64
                        object.real_coords.y = row.get::<_, f64>(4).unwrap() as i64; //i64
                        object.real_coords.z = row.get::<_, f64>(5).unwrap() as i64; //i64
                        object.projected_coords.x = row.get::<_, f64>(6).unwrap() as i64; //i64
                        object.projected_coords.y = row.get::<_, f64>(7).unwrap() as i64; //i64

                        // Invert coordinates if needed
                        if invert_coordinates {
                            object.real_coords.x *= -1;
                            object.real_coords.y *= -1;
                            object.real_coords.z *= -1;
                            object.projected_coords.x *= -1;
                            object.projected_coords.y *= -1;
                        }
                        object.region = row.get(2).unwrap();
                        temp_vec.push(object);
                    }
                    if vec_parent_ids.len() == 0 {
                        break;
                    };
                }
                let mut query = String::from(" SELECT msg.solarSystemId FROM mapSystemGates ");
                query += " AS msg WHERE msg.systemGateId ";
                query +=
                    " IN (SELECT destination FROM mapSystemGates AS msg WHERE solarSystemId = ?)";
                for mut object in temp_vec {
                    let mut statement = thread_connection.prepare(query.as_str()).unwrap();
                    let mut rows = statement.query([object.id]).unwrap();
                    while let Some(row) = rows.next().unwrap() {
                        object.connections.push(row.get(0).unwrap());
                    }
                    sh_objects.lock().unwrap().push(object);
                }
            });
            // store the handles
            handles.push(handle);
        }

        // Initialize Constellation Vector
        let mut vec_result = vec![];
        for handle in handles {
            //Waiting the threads to end
            handle.join().unwrap();
            // Getting the Object vector
            let vec = &mut vec_objects.lock().unwrap();
            vec_result.append(vec);
        }
        Ok(vec_result)
    }

    /// Function to get every Constellation or a Constellation based on an specific Region
    pub fn get_constellation(
        &self,
        connection: rusqlite::Connection,
        regions: Option<Vec<u32>>,
    ) -> Result<Vec<Constellation>, RusqliteError> {
        #[cfg(feature = "puffin")]
        puffin::profile_scope!("get_constellation");
        // preparing the connections that will be shared between threads
        let kconn = Arc::new(Mutex::new(connection));
        let mut handles = vec![];

        //Preparing the Mutexed Vector to get all constellations
        let vec_objects = Arc::new(Mutex::new(Vec::new()));
        // Preparing a Mutexed Vector with region ids data
        let vec_parent_ids = match regions {
            Some(temp_vec) => Arc::new(Mutex::new(temp_vec)),
            None => Arc::new(Mutex::new(vec![])),
        };
        for _x in (0..consts::MAX_THREADS).collect::<std::vec::Vec<i8>>() {
            // cloning Objects to invoke a thread
            let sh_objects = Arc::clone(&vec_objects);
            let sh_parent_ids = Arc::clone(&vec_parent_ids);
            let sh_conn = Arc::clone(&kconn);

            // invoke a thread
            let handle = thread::spawn(move || {
                let thread_connection = &sh_conn.lock().unwrap();
                let mut query = String::from(
                    "SELECT constellationId, constellationName, regionId FROM mapConstellations",
                );
                let vec_parent_ids = &mut sh_parent_ids.lock().unwrap();
                if vec_parent_ids.len() > 0 {
                    query += " WHERE regionId=?";
                };
                let mut temp_vec = Vec::new();
                loop {
                    let mut statement = thread_connection.prepare(query.as_str()).unwrap();
                    let mut rows = statement.query([&vec_parent_ids.pop().unwrap()]).unwrap();
                    //while there are regions left to consume
                    while let Some(row) = rows.next().unwrap() {
                        let mut object = Constellation::new();
                        object.id = row.get(0).unwrap();
                        object.name = row.get(1).unwrap();
                        object.region = row.get(2).unwrap();
                        temp_vec.push(object);
                    }
                    if vec_parent_ids.len() == 0 {
                        break;
                    };
                }
                let query = "SELECT solarSystemId FROM mapSolarSystems WHERE constellationId = ?";
                for mut object in temp_vec {
                    let mut statement = thread_connection.prepare(query).unwrap();
                    let mut rows = statement.query([object.id]).unwrap();
                    while let Some(row) = rows.next().unwrap() {
                        object.solar_systems.push(row.get(0).unwrap());
                    }
                    sh_objects.lock().unwrap().push(object);
                }
            });
            // store the handles
            handles.push(handle);
        }

        // Initialize Constellation Vector
        let mut vec_result = vec![];
        for handle in handles {
            //Waiting the threads to end
            handle.join().unwrap();
            // Getting the Object vector
            let vec = &mut vec_objects.lock().unwrap();
            vec_result.append(vec);
        }
        Ok(vec_result)
    }

    /// Function to get every Planet or all Planets for a specific Solar System
    pub fn get_planet(
        &self,
        connection: rusqlite::Connection,
        solar_systems: Option<Vec<u32>>,
    ) -> Result<Vec<Planet>, RusqliteError> {
        #[cfg(feature = "puffin")]
        puffin::profile_scope!("get_planet");
        // preparing the connections that will be shared between threads
        let kconn = Arc::new(Mutex::new(connection));
        let mut handles = vec![];

        //Preparing the Mutexed Vector to get all constellations
        let vec_objects = Arc::new(Mutex::new(Vec::new()));
        // Preparing a Mutexed Vector with region ids data
        let vec_parent_ids = match solar_systems {
            Some(temp_vec) => Arc::new(Mutex::new(temp_vec)),
            None => Arc::new(Mutex::new(vec![])),
        };
        for _x in (0..consts::MAX_THREADS).collect::<std::vec::Vec<i8>>() {
            // cloning Objects to invoke a thread
            let sh_objects = Arc::clone(&vec_objects);
            let sh_parent_ids = Arc::clone(&vec_parent_ids);
            let sh_conn = Arc::clone(&kconn);

            // invoke a thread
            let handle = thread::spawn(move || {
                let thread_connection = &sh_conn.lock().unwrap();
                let mut query =
                    String::from("SELECT planetId, planetaryIndex, solarSystemId FROM mapPlanets");
                let vec_parent_ids = &mut sh_parent_ids.lock().unwrap();
                if vec_parent_ids.len() > 0 {
                    query += " WHERE solarSystemId=?";
                };
                loop {
                    let mut statement = thread_connection.prepare(query.as_str()).unwrap();
                    let mut rows = statement.query([&vec_parent_ids.pop().unwrap()]).unwrap();
                    //while there are regions left to consume
                    while let Some(row) = rows.next().unwrap() {
                        let mut object = Planet::new();
                        object.id = row.get(0).unwrap();
                        object.solar_system = row.get(2).unwrap();
                        object.index = row.get(1).unwrap();
                        sh_objects.lock().unwrap().push(object);
                    }
                    if vec_parent_ids.len() == 0 {
                        break;
                    };
                }
            });
            // store the handles
            handles.push(handle);
        }

        // Initialize Constellation Vector
        let mut vec_result = vec![];
        for handle in handles {
            //Waiting the threads to end
            handle.join().unwrap();
            // Getting the Object vector
            let vec = &mut vec_objects.lock().unwrap();
            vec_result.append(vec);
        }
        Ok(vec_result)
    }

    /// Function to get every Moon or all Moons for a specific planet
    pub fn get_moon(
        &self,
        connection: rusqlite::Connection,
        planets: Option<Vec<u32>>,
    ) -> Result<Vec<Moon>, RusqliteError> {
        #[cfg(feature = "puffin")]
        puffin::profile_scope!("get_moon");

        // preparing the connections that will be shared between threads
        let kconn = Arc::new(Mutex::new(connection));
        let mut handles = vec![];

        //Preparing the Mutexed Vector to get all constellations
        let vec_objects = Arc::new(Mutex::new(Vec::new()));
        // Preparing a Mutexed Vector with region ids data
        let vec_parent_ids = match planets {
            Some(temp_vec) => Arc::new(Mutex::new(temp_vec)),
            None => Arc::new(Mutex::new(vec![])),
        };
        for _x in (0..consts::MAX_THREADS).collect::<std::vec::Vec<i8>>() {
            // cloning Objects to invoke a thread
            let sh_objects = Arc::clone(&vec_objects);
            let sh_parent_ids = Arc::clone(&vec_parent_ids);
            let sh_conn = Arc::clone(&kconn);

            // invoke a thread
            let handle = thread::spawn(move || {
                let thread_connection = &sh_conn.lock().unwrap();
                let mut query = String::from(
                    "SELECT moonId, moonIndex, solarSystemId, planetId FROM mapMoons ",
                );
                let vec_parent_ids = &mut sh_parent_ids.lock().unwrap();
                if vec_parent_ids.len() > 0 {
                    query += " WHERE planetId=?";
                };
                loop {
                    let mut statement = thread_connection.prepare(query.as_str()).unwrap();
                    let mut rows = statement.query([&vec_parent_ids.pop().unwrap()]).unwrap();
                    //while there are regions left to consume
                    while let Some(row) = rows.next().unwrap() {
                        let mut object = Moon::new();
                        object.id = row.get(0).unwrap();
                        object.planet = row.get(3).unwrap();
                        object.index = row.get(1).unwrap();
                        object.solar_system = row.get(2).unwrap();
                        sh_objects.lock().unwrap().push(object);
                    }
                    if vec_parent_ids.len() == 0 {
                        break;
                    };
                }
            });
            // store the handles
            handles.push(handle);
        }

        // Initialize Constellation Vector
        let mut vec_result = vec![];
        for handle in handles {
            //Waiting the threads to end
            handle.join().unwrap();
            // Getting the Object vector
            let vec = &mut vec_objects.lock().unwrap();
            vec_result.append(vec);
        }
        Ok(vec_result)
    }
}

impl Default for Universe {
    fn default() -> Self {
        Self::new(1)
    }
}
