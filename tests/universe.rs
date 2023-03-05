#[cfg(test)]
mod universe_tests {
    //use super::*;
    use std::path::Path;
    use sde::SdeManager;
    use async_std::task;
    

    #[test]
    fn test_solar_systems() {
        let path = Path::new("tests/sde.db");
        let mut manager = sde::SdeManager::new(path);
        let _resp = manager.get_universe();
        assert_eq!(manager.universe.solar_systems.len(), 5431);
    }

    #[test]
    fn test_regions() {
        let path = Path::new("tests/sde.db");
        let mut manager = sde::SdeManager::new(path);
        let _resp= manager.get_universe();
        assert_eq!(manager.universe.regions.len(), 68);
    }


    #[test]
    fn test_spatialpoints() {
        let path = Path::new("tests/sde.db");
        let manager = sde::SdeManager::new(path);
        assert!(manager.get_points().unwrap());
    }


    #[test]
    fn test_async_constellations() -> () {
        let path = Path::new("assets/sde-isometric.db");
        let mut a = SdeManager::new(path);
        task::spawn(async move {
            let _res = &a.get_async_universe().await;
            assert_eq!(a.universe.constellations.len(), 789);
        });  
        
    } 
    
}
