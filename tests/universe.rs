#[cfg(test)]
mod universe_tests {
    //use super::*;
    use std::path::Path;

    #[test]
    fn test_solar_systems() {
        let path = Path::new("tests/sde.db");
        let mut manager = sde::SdeManager::new(path, 100000000000000);
        let _resp = manager.get_universe();
        assert_eq!(manager.universe.solar_systems.len(), 5431);
    }

    #[test]
    fn test_regions() {
        let path = Path::new("tests/sde.db");
        let mut manager = sde::SdeManager::new(path, 100000000000000);
        let _resp= manager.get_universe();
        assert_eq!(manager.universe.regions.len(), 68);
    }

    #[test]
    fn test_constellations() {
        let path = Path::new("tests/sde.db");
        let mut manager = sde::SdeManager::new(path, 100000000000000);
        let _resp= manager.get_universe();
        assert_eq!(manager.universe.constellations.len(), 789);
    }


    #[test]
    fn test_3dpoints() {
        let path = Path::new("tests/sde.db");
        let manager = sde::SdeManager::new(path, 100000000000000);
        assert_eq!(manager.get_systempoints(2).unwrap().len(),5431);
    }
    
}
