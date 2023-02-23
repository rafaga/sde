use std::collections::HashMap;
use std::sync::{Arc,Mutex};
use std::thread;
/*use kdtree::kdtree::Kdtree;
use kdtree::kdtree::distance::squared_euclidean;*/
use kdtree::kdtree::KdtreePointTrait;
use sqlite::Error;
use super::consts;

// This can by any object or point with its associated metadata
/// Struct that contains coordinates to help calculate nearest point in space
#[derive(Copy, Clone, PartialEq)]
pub struct KdtreePoint {
    dims: [f64; 3],
    /// Object Identifier for search propurses
    pub id:i32,
}

impl KdtreePointTrait for KdtreePoint{
    #[inline] // the inline on this method is important! as without it there is ~25% speed loss on the tree when cross-crate usage.
    fn dims(&self) -> &[f64] {
        &self.dims
    }
}

#[derive(Hash, PartialEq, Eq)]
/// 3d point coordinates that it is used in:
/// 
/// - SolarSystems
pub struct Coordinates {
    /// X coorddinate
    pub x: i64,
    /// Y coordinate
    pub y: i64,
    /// Z coordinate
    pub z: i64
}

impl Coordinates {
    /// Creates a new Coordinates struct. ALl the coordinates are initialized. 
    pub fn new() -> Self{
        Coordinates {
            x:0,
            y:0,
            z:0,
        }
    }
}

impl Default for Coordinates {
    fn default() -> Self {
        Self::new()
    }
}

/// 2d point coordinates that it is used in:
/// 
/// - SolarSystems (to represent abstraction maps)
#[derive(Hash, PartialEq, Eq)]
pub struct AbstractCoordinates {
    /// X coordinate
    pub x: i32,
    /// Y coordinate
    pub y: i32
}


impl AbstractCoordinates {
    /// Creates a new AbstractCoordinate struct. ALl the coordinates are initialized. 
    pub fn new() -> Self{
        AbstractCoordinates {
            x:0,
            y:0,
        }
    }
}

impl Default for AbstractCoordinates {
    fn default() -> Self {
        Self::new()
    }
}

/// Abstraction for a Planet Moons. It store data relevant to this entity
#[derive(Hash, PartialEq, Eq)]
pub struct Moon{
    /// Moon Identifier
    pub id: u32,
    /// Moon's Planet identifier 
    pub planet: u32,
    /// The cardinal number of this moon in the planet
    pub index: u8,
    /// Moon's Solar System Identifier
    pub solar_system: u32,
}

impl Moon{
    /// Creates a new Moon Strcut. ALl the values are initialized. Needs to be filled
    pub fn new() -> Self{
        Moon {
            id: 0, 
            planet: 0,
            index: 0,
            solar_system:0,
        }
    }
}

impl Default for Moon {
    fn default() -> Self {
        Self::new()
    }
}

/// Abstraction for a Planet. It store data relevant to this entity
#[derive(Hash, PartialEq, Eq)]
pub struct Planet{
    /// Planet identifier
    pub id: u32,
    /// Planet's Solar System Idetifier
    pub solar_system: u32,
    /// The cardinal number of this planet in the solar system.
    pub index: u8
}

impl Planet{
    /// Creates a new Planet Strcut. ALl the values are initialized. Needs to be filled
    pub fn new() -> Self{
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
#[derive(Hash, PartialEq, Eq)]
pub struct SolarSystem{
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
    pub cords3d: Coordinates,
    /// Solar System 2D Coordinates with the propourse of representing the system in abstraction map.
    pub cords2d: AbstractCoordinates,
}

impl SolarSystem{
    /// Creates a new Solar System Strcut. ALl the values are initialized. Needs to be filled
    pub fn new() -> Self{
        SolarSystem {
            id: 0, 
            name:String::new(),
            region:0,
            constellation: 0,
            planets: Vec::new(),
            connections: Vec::new(),
            cords3d: Coordinates::default(),
            cords2d: AbstractCoordinates::default(),
        }
    }
}

impl Default for SolarSystem {
    fn default() -> Self {
        Self::new()
    }
}

/// Abstraction for a Constellation. It store data relevant to this entity
#[derive(Hash, PartialEq, Eq)]
pub struct Constellation{
    /// Constellation Identifier
    pub id: u32,
    /// Constellation Name
    pub name: String,
    /// Region Identifier
    pub region: u32,
    /// Solar System vector with Identifer numbers included in the constellation
    pub solar_systems: Vec<u32>,
    /// Solar System 2D Coordinates with the propourse of representing the system in abstraction map.
    pub cords2d: AbstractCoordinates
}

impl Constellation{
    /// Creates a new Constellation Strcut. ALl the values are initialized. Needs to be filled
    pub fn new() -> Self{
        Constellation {
            id:0,
            name:String::new(), 
            region: 0,
            solar_systems: Vec::new(),
            cords2d: AbstractCoordinates::default(),
        }
    }
}

impl Default for Constellation {
    fn default() -> Self {
        Self::new()
    }
}

/// Abstraction for a Region. It store data relevant to this entity
#[derive(Hash, PartialEq, Eq)]
pub struct Region{
    /// Region Identifier
    pub id: u32,
    /// Region Name
    pub name: String,
    /// Vector with Region's Constellationm Identifiers
    pub constellations: Vec<u32>,
     /// Region 2D Coordinates with the propourse of representing the system in abstraction map.
    pub cords2d: AbstractCoordinates
}

impl Region{
    /// Creates a new Region Strcut. ALl the values are initialized. Needs to be filled
    pub fn new() -> Self{
        Region{
            id:0, 
            name:String::new(), 
            constellations:Vec::new(),
            cords2d: AbstractCoordinates::default(),
        }
    }
}

impl Default for Region {
    fn default() -> Self {
        Self::new()
    }
}
 
/// Struct that contains Dictionary to search regions, constellations and solarsystems by name
pub struct Dictionaries{
    /// Solar system dictionary
    pub system_names: HashMap<String,u32>,
    /// Constellations dictionary
    pub constellation_names: HashMap<String,u32>,
    /// Region dictionary
    pub region_names: HashMap<String,u32>,
}

impl Dictionaries{
    /// Creates a new Dictionaries Strcut. ALl the values are initialized. Needs to be filled
    pub fn new() -> Dictionaries{
        Dictionaries{
            system_names:HashMap::new(),
            constellation_names:HashMap::new(),
            region_names:HashMap::new(),
        }
    }
}

impl Default for Dictionaries {
    fn default() -> Self {
        Self::new()
    }
}

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
    pub regions: HashMap<u32,Region>,
    /// Constellation objects you can access the data with their Identfiers
    pub constellations: HashMap<u32,Constellation>,
    /// Solarsystem objects you can access the data with their Identfiers
    pub solar_systems: HashMap<u32, SolarSystem>,
    /// Planet objects you can access the data with their Identfiers
    pub planets: HashMap<u32, Planet>,
    /// Moon objects you can access the data with their Identfiers
    pub moons: HashMap<u32, Moon>,
    /// Dictionaries struct
    pub dicts: Dictionaries,
}

impl Universe{

    /// Creates a new Universe Strcut. ALl the values are initialized. Needs to be filled
    pub fn new() -> Universe{
        Universe{
            regions:HashMap::new(),
            constellations:HashMap::new(),
            solar_systems:HashMap::new(),
            planets:HashMap::new(),
            moons:HashMap::new(),
            dicts:Dictionaries::new(),
        }
    }

    /// Function to get every region available in SDE database
    pub fn get_region(&self, connection:&sqlite::ConnectionWithFullMutex,  regions: Option<Vec<u32>>) -> Result<Vec<Region>,Error>{
        let mut query = String::from("SELECT regionId, regionName FROM mapRegions");
        if let Some(..) = regions{
            query += " WHERE regionId=?;";
        } 
        
        let mut statement = connection.prepare(query)?;
        /*if let Some(region) = regions{
            statement.bind((1,region as i64)).unwrap();
        }*/
        let mut temp_regions = Vec::new();
        while let Ok(sqlite::State::Row) = statement.next() {
            let mut region = Region::new();
            region.id=statement.read::<i64, _>("regionId").unwrap() as u32; 
            region.name=statement.read::<String, _>("regionName").unwrap();
            temp_regions.push(region);
        };
        let query = "SELECT constellationId FROM mapConstellations WHERE regionId=?";
        let mut regions = Vec::new();
        for mut region in temp_regions{
            let mut statement = connection.prepare(query)?;
            statement.bind((1,region.id as i64)).unwrap();
            while let Ok(sqlite::State::Row) = statement.next() {
                region.constellations.push(statement.read::<i64, _>("constellationId").unwrap() as u32);
            }
            regions.push(region);
        }
        Ok(regions)
    }

    /// Function to get every Solarsystem or a Solar systems for a specific Constellation
    pub fn get_solarsystem(&self,  connection:sqlite::ConnectionWithFullMutex,  constellation: Option<Vec<u32>>) -> Result<Vec<SolarSystem>,Error>{
        // preparing the connections that will be shared between threads
        let kconn = Arc::new(Mutex::new(connection));
        let mut handles = vec![];

        //Preparing the Mutexed Vector to get all constellations
        let vec_objects = Arc::new(Mutex::new(Vec::new()));
        // Preparing a Mutexed Vector with region ids data
        let vec_parent_ids = match constellation{
            Some(temp_vec) => Arc::new(Mutex::new(temp_vec)),
            None => Arc::new(Mutex::new(vec![])),
        };
        for _x in [0..consts::MAX_THREADS] {
            // cloning Objects to invoke a thread
            let sh_objects = Arc::clone(&vec_objects);
            let sh_parent_ids = Arc::clone(&vec_parent_ids);
            let sh_conn = Arc::clone(&kconn);

            // invoke a thread
            let handle = thread::spawn(move || {
                let thread_connection = &sh_conn.lock().unwrap();
                let mut query = String::from("SELECT mss.solarSystemId, mss.solarSystemName, mc.regionId, ");
                query += " mc.centerX, mc.centerY, mc.centerZ, mas.x, mas.y, mss.constellationId ";
                query += " FROM mapSolarSystems AS mss INNER JOIN mapConstellations AS mc ";
                query += " ON(mss.constellationId = mc.constellationId) INNER JOIN mapAbstractSystems AS mas ";
                query += " ON(mas.solarSystemId = mss.solarSystemId) ";
                let vec_parent_ids = &mut sh_parent_ids.lock().unwrap();
                if vec_parent_ids.len() > 0 {
                    query += " WHERE mss.constellationId=? ";
                };
                let mut temp_vec = Vec::new();
                loop{
                    let mut statement = thread_connection.prepare(&query).unwrap();
                    if vec_parent_ids.len() > 0 {
                        statement.bind((1,vec_parent_ids.pop().unwrap() as i64)).unwrap();
                    }
                     //while there are constellations left to consume
                    while let Ok(sqlite::State::Row) = statement.next() {
                        let mut object = SolarSystem::new();
                        object.id=statement.read::<i64, _>("solarSystemId").unwrap() as u32;
                        object.name=statement.read::<String, _>("solarSystemName").unwrap();
                        object.constellation =statement.read::<i64, _>("constellationId").unwrap() as u32;
                        object.cords3d.x = statement.read::<i64, _>("centerX").unwrap();
                        object.cords3d.y = statement.read::<i64, _>("centerY").unwrap();
                        object.cords3d.z = statement.read::<i64, _>("centerZ").unwrap();
                        object.cords2d.x = statement.read::<i64, _>("x").unwrap() as i32;
                        object.cords2d.y = statement.read::<i64, _>("y").unwrap() as i32;
                        object.region = statement.read::<i64, _>("regionId").unwrap() as u32;
                        temp_vec.push(object);
                    };
                    if vec_parent_ids.len() == 0{
                        break;
                    };
                }
                let mut query = String::from(" SELECT msg.solarSystemId FROM mapSystemGates ");
                query += " AS msg WHERE msg.systemGateId ";
                query += " IN (SELECT destination FROM mapSystemGates AS msg WHERE solarSystemId = ?)";
                for mut object in temp_vec {
                    let mut statement = thread_connection.prepare(&query).unwrap();
                    statement.bind((1,object.id as i64)).unwrap();
                    while let Ok(sqlite::State::Row) = statement.next() {
                        object.connections.push(statement.read::<i64, _>("solarSystemId").unwrap() as u32);
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
    pub fn get_constellation(&self,  connection:sqlite::ConnectionWithFullMutex,  regions: Option<Vec<u32>>) -> Result<Vec<Constellation>,Error>{
        // preparing the connections that will be shared between threads
        let kconn = Arc::new(Mutex::new(connection));
        let mut handles = vec![];

        //Preparing the Mutexed Vector to get all constellations
        let vec_objects = Arc::new(Mutex::new(Vec::new()));
        // Preparing a Mutexed Vector with region ids data
        let vec_parent_ids = match regions{
            Some(temp_vec) => Arc::new(Mutex::new(temp_vec)),
            None => Arc::new(Mutex::new(vec![])),
        };
        for _x in [0..consts::MAX_THREADS] {
            // cloning Objects to invoke a thread
            let sh_objects = Arc::clone(&vec_objects);
            let sh_parent_ids = Arc::clone(&vec_parent_ids);
            let sh_conn = Arc::clone(&kconn);

            // invoke a thread
            let handle = thread::spawn(move || {
                let thread_connection = &sh_conn.lock().unwrap();
                let mut query = String::from("SELECT constellationId, constellationName, regionId FROM mapConstellations");
                let vec_parent_ids = &mut sh_parent_ids.lock().unwrap();
                if vec_parent_ids.len() > 0 {
                    query += " WHERE regionId=?";
                };
                let mut temp_vec = Vec::new();
                loop{
                    let mut statement = thread_connection.prepare(&query).unwrap();
                    if vec_parent_ids.len() > 0 {
                        statement.bind((1,vec_parent_ids.pop().unwrap() as i64)).unwrap();
                    }
                    //while there are regions left to consume
                    while let Ok(sqlite::State::Row) = statement.next() {
                        let mut object = Constellation::new();
                        object.id = statement.read::<i64, _>("constellationId").unwrap() as u32; 
                        object.name = statement.read::<String, _>("constellationName").unwrap();
                        object.region = statement.read::<i64, _>("regionId").unwrap() as u32;
                        temp_vec.push(object);
                    };
                    if vec_parent_ids.len() == 0{
                        break;
                    };
                }
                let query = "SELECT solarSystemId FROM mapSolarSystems WHERE constellationId = ?";
                for mut object in temp_vec{
                    let mut statement = thread_connection.prepare(&query).unwrap();
                    statement.bind((1,object.id as i64)).unwrap();
                    while let Ok(sqlite::State::Row) = statement.next() {
                        object.solar_systems.push(statement.read::<i64, _>("solarSystemId").unwrap() as u32); 
                    };
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
    pub fn get_planet(&self,  connection:sqlite::ConnectionWithFullMutex,  solar_systems: Option<Vec<u32>>) -> Result<Vec<Planet>,Error>{
        // preparing the connections that will be shared between threads
        let kconn = Arc::new(Mutex::new(connection));
        let mut handles = vec![];

        //Preparing the Mutexed Vector to get all constellations
        let vec_objects = Arc::new(Mutex::new(Vec::new()));
        // Preparing a Mutexed Vector with region ids data
        let vec_parent_ids = match solar_systems{
            Some(temp_vec) => Arc::new(Mutex::new(temp_vec)),
            None => Arc::new(Mutex::new(vec![])),
        };
        for _x in [0..consts::MAX_THREADS] {
            // cloning Objects to invoke a thread
            let sh_objects = Arc::clone(&vec_objects);
            let sh_parent_ids = Arc::clone(&vec_parent_ids);
            let sh_conn = Arc::clone(&kconn);

            // invoke a thread
            let handle = thread::spawn(move || {
                let thread_connection = &sh_conn.lock().unwrap();
                let mut query = String::from("SELECT planetId, planetaryIndex, solarSystemId FROM mapPlanets");
                let vec_parent_ids = &mut sh_parent_ids.lock().unwrap();
                if vec_parent_ids.len() > 0 {
                    query += " WHERE solarSystemId=?";
                };
                loop{
                    let mut statement = thread_connection.prepare(&query).unwrap();
                    if vec_parent_ids.len() > 0 {
                        statement.bind((1,vec_parent_ids.pop().unwrap() as i64)).unwrap();
                    }
                     //while there are regions left to consume
                    while let Ok(sqlite::State::Row) = statement.next() {
                        let mut object = Planet::new();
                        object.id=statement.read::<i64, _>("planetId").unwrap() as u32; 
                        object.solar_system = statement.read::<i64, _>("solarSystemId").unwrap() as u32;
                        object.index=statement.read::<i64, _>("planetaryIndex").unwrap() as u8;
                        sh_objects.lock().unwrap().push(object);
                    };
                    if vec_parent_ids.len() == 0{
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
    pub fn get_moon(&self,  connection:sqlite::ConnectionWithFullMutex,  planets: Option<Vec<u32>>) -> Result<Vec<Moon>,Error>{
        // preparing the connections that will be shared between threads
        let kconn = Arc::new(Mutex::new(connection));
        let mut handles = vec![];

        //Preparing the Mutexed Vector to get all constellations
        let vec_objects = Arc::new(Mutex::new(Vec::new()));
        // Preparing a Mutexed Vector with region ids data
        let vec_parent_ids = match planets{
            Some(temp_vec) => Arc::new(Mutex::new(temp_vec)),
            None => Arc::new(Mutex::new(vec![])),
        };
        for _x in [0..consts::MAX_THREADS] {
            // cloning Objects to invoke a thread
            let sh_objects = Arc::clone(&vec_objects);
            let sh_parent_ids = Arc::clone(&vec_parent_ids);
            let sh_conn = Arc::clone(&kconn);

            // invoke a thread
            let handle = thread::spawn(move || {
                let thread_connection = &sh_conn.lock().unwrap();
                let mut query = String::from("SELECT moonId, moonIndex, solarSystemId, planetId FROM mapMoons ");
                let vec_parent_ids = &mut sh_parent_ids.lock().unwrap();
                if vec_parent_ids.len() > 0 {
                    query += " WHERE planetId=?";
                };
                loop{
                    let mut statement = thread_connection.prepare(&query).unwrap();
                    if vec_parent_ids.len() > 0 {
                        statement.bind((1,vec_parent_ids.pop().unwrap() as i64)).unwrap();
                    }
                        //while there are regions left to consume
                    while let Ok(sqlite::State::Row) = statement.next() {
                        let mut object = Moon::new();
                        object.id=statement.read::<i64, _>("moonId").unwrap() as u32; 
                        object.planet = statement.read::<i64, _>("planetId").unwrap() as u32;
                        object.index=statement.read::<i64, _>("moonIndex").unwrap() as u8; 
                        object.solar_system=statement.read::<i64, _>("solarSystemId").unwrap() as u32;
                        sh_objects.lock().unwrap().push(object);
                    };
                    if vec_parent_ids.len() == 0{
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
        Self::new()
    }
}