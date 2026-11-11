//! Systems without stargates (the Jove regions) must be listed by
//! `get_systems` and `get_abstract_systems`, with no connections.
//!
//! Runs against a real database pointed to by `SDE_TEST_DB` and is a
//! no-op when the variable is not set.

use sde::SdeManager;
use std::path::PathBuf;

/// UUA-F4, J7HZ-F and A821-A: the three Jove regions.
const JOVE_REGIONS: [u32; 3] = [10000004, 10000017, 10000019];

/// Opens the database named by `SDE_TEST_DB`, if any.
struct Fixture;

impl Fixture {
    fn path() -> Option<PathBuf> {
        std::env::var_os("SDE_TEST_DB").map(PathBuf::from)
    }
}

#[test]
fn universe_lists_systems_without_gates() {
    let Some(path) = Fixture::path() else { return };
    let manager = SdeManager::new(&path, 1.0).unwrap();
    let systems = manager.get_systems().unwrap();
    let gateless = systems
        .values()
        .filter(|point| point.connections.is_empty())
        .count();
    assert!(systems.len() >= 5485, "got {}", systems.len());
    assert!(gateless >= 217, "got {gateless}");
    assert!(
        systems
            .values()
            .filter(|point| !point.connections.is_empty())
            .count()
            >= 5268
    );
}

#[test]
fn jove_regions_list_every_system() {
    let Some(path) = Fixture::path() else { return };
    let manager = SdeManager::new(&path, 1.0).unwrap();
    let systems = manager.get_abstract_systems(JOVE_REGIONS.to_vec()).unwrap();
    let gateless = systems
        .values()
        .filter(|point| point.connections.is_empty())
        .count();
    assert!(gateless >= 217, "got {gateless}");
}

#[test]
fn gated_regions_keep_their_connections() {
    let Some(path) = Fixture::path() else { return };
    let manager = SdeManager::new(&path, 1.0).unwrap();
    // The Forge
    let systems = manager.get_abstract_systems(vec![10000002]).unwrap();
    assert!(!systems.is_empty());
    assert!(systems.values().any(|point| !point.connections.is_empty()));
    let lone = systems
        .values()
        .filter(|point| point.connections.is_empty())
        .count();
    assert_eq!(lone, 0);
}

#[test]
fn thera_is_the_only_system_without_an_abstract_position_in_its_region() {
    let Some(path) = Fixture::path() else { return };
    let manager = SdeManager::new(&path, 1.0).unwrap();
    let unplaced = manager.get_unplaced_systems(vec![11000031]).unwrap();
    let thera = unplaced.get(&31000005).expect("Thera");
    assert_eq!(thera.name.as_deref(), Some("Thera"));
    assert_eq!(thera.coords, [0.0, 0.0, 0.0]);
    assert!(thera.connections.is_empty());
    // Every system of a mapped region is on its abstract map already.
    assert!(
        manager
            .get_unplaced_systems(vec![10000002])
            .unwrap()
            .is_empty()
    );
    assert!(manager.get_unplaced_systems(Vec::new()).unwrap().is_empty());
}

#[test]
fn wormhole_class_lives_in_the_region_and_the_systems_take_it() {
    let Some(path) = Fixture::path() else { return };
    let mut manager = SdeManager::new(&path, 1.0).unwrap();
    manager.get_universe().unwrap();
    let universe = &manager.universe;
    // The class is a property of the region ...
    assert_eq!(universe.regions[&11000031].wormhole_class, Some(12));
    assert_eq!(universe.regions[&10000002].wormhole_class, Some(7));
    // ... a system has one of its own only when it is a drifter ...
    assert_eq!(universe.solar_systems[&31000005].wormhole_class, None);
    assert_eq!(universe.solar_systems[&31000001].wormhole_class, Some(14));
    // ... and `Universe::wormhole_class` gives the one that applies.
    assert_eq!(universe.wormhole_class(31000005), Some(12));
    assert_eq!(universe.wormhole_class(31000001), Some(14));
    let class_of = |name: &str| {
        let system = universe
            .solar_systems
            .values()
            .find(|system| system.name == name)?;
        universe.wormhole_class(system.id)
    };
    assert_eq!(class_of("J001057"), Some(2));
    assert_eq!(class_of("J001348"), Some(3));
    assert_eq!(universe.wormhole_class(1), None);
}

#[test]
fn the_summaries_say_what_the_universe_says() {
    let Some(path) = Fixture::path() else { return };
    let mut manager = SdeManager::new(&path, 1.0).unwrap();
    manager.get_universe().unwrap();
    let universe = &manager.universe;
    let summaries = manager.get_system_summaries().unwrap();
    assert_eq!(summaries.len(), universe.solar_systems.len());
    for (id, system) in &universe.solar_systems {
        let summary = &summaries[id];
        assert_eq!(summary.name, system.name);
        assert_eq!(summary.region_id, system.region);
        assert_eq!(summary.region_name, universe.regions[&system.region].name);
        assert_eq!(summary.security, system.security);
        assert_eq!(summary.wormhole_class, universe.wormhole_class(*id));
    }
}
