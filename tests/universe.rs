#[cfg(test)]
mod universe_tests {
    //use super::*;
    use std::path::Path;

    #[test]
    fn test_solar_systems(){
        let path = Path::new("tests/sde.db");
        let manager = sde::SdeManager::new(path);
        let mut univ = sde::objects::Universe::new();
        manager.get_universe(&mut univ).unwrap();
        assert_eq!(univ.solar_systems.len(),5431);
    }

    #[test]
    fn test_regions(){
        let path = Path::new("tests/sde.db");
        let manager = sde::SdeManager::new(path);
        let mut univ = sde::objects::Universe::new();
        manager.get_universe(&mut univ).unwrap();
        assert_eq!(univ.regions.len(),68);
    }

    #[test]
    fn test_constellations(){
        let path = Path::new("tests/sde.db");
        let manager = sde::SdeManager::new(path);
        let mut univ = sde::objects::Universe::new();
        manager.get_universe(&mut univ).unwrap();
        assert_eq!(univ.constellations.len(),789);
    }

}