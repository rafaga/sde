use super::{Constellation, Moon, Planet, Region, SolarSystem};
use std::collections::HashMap;

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
    /// Factor used to correct coordinates
    pub factor: f64,
}

impl Universe {
    /// Creates a new Universe Strcut. ALl the values are initialized. Needs to be filled
    pub fn new(factor: f64) -> Universe {
        Universe {
            regions: HashMap::new(),
            constellations: HashMap::new(),
            solar_systems: HashMap::new(),
            planets: HashMap::new(),
            moons: HashMap::new(),
            factor,
        }
    }

    /// The wormhole class that applies to a system: its own if it has one (the
    /// drifter systems) or else the one of its region. `None` if the system is
    /// not in the universe or neither has one.
    pub fn wormhole_class(&self, system_id: u32) -> Option<u32> {
        let system = self.solar_systems.get(&system_id)?;
        system.wormhole_class.or_else(|| {
            self.regions
                .get(&system.region)
                .and_then(|region| region.wormhole_class)
        })
    }
}

impl Default for Universe {
    fn default() -> Self {
        Self::new(1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn universe_new_initializes_with_factor() {
        let universe = Universe::new(42.0);
        assert!(universe.regions.is_empty());
        assert!(universe.constellations.is_empty());
        assert!(universe.solar_systems.is_empty());
        assert!(universe.planets.is_empty());
        assert!(universe.moons.is_empty());
        assert_eq!(universe.factor, 42.0);
    }

    #[test]
    fn universe_default_factor_is_one() {
        assert_eq!(Universe::default().factor, 1.0);
    }
}
