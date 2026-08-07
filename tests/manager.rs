//! Integration tests for `SdeManager` using a minimal SQLite fixture that
//! mimics the SDE database schema used by the crate's queries.
//!
//! The fixture contains:
//! - 2 regions (10000001 "Region Alpha", 10000002 "Region Beta")
//! - 2 constellations (one per region)
//! - 4 solar systems (3 in K-Space range 30000000..=30999999, 1 outside)
//! - 2 stargate connections (1-2 and 2-3)
//! - 3 planets and 1 moon
//! - 3 abstract systems (2 in Region Alpha, 1 in Region Beta)

use rusqlite::Connection;
use sde::SdeManager;
use sde::objects::{MapPoint, map_points_to_vec};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Factor used by most tests: coordinates are divided by 100.
const FACTOR: i64 = 100;

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Creates a temporary SDE-like database and removes it on drop.
struct Fixture {
    path: PathBuf,
}

impl Fixture {
    fn new(test_name: &str) -> Self {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "sde_test_{}_{}_{}.db",
            test_name,
            std::process::id(),
            id
        ));
        let conn = Connection::open(&path).expect("cannot create fixture database");
        conn.execute_batch(
            "
            CREATE TABLE mapRegions (regionId INTEGER PRIMARY KEY, regionName TEXT NOT NULL);
            CREATE TABLE mapConstellations (
                constellationId INTEGER PRIMARY KEY,
                constellationName TEXT NOT NULL,
                regionId INTEGER NOT NULL,
                centerX REAL, centerY REAL, centerZ REAL
            );
            CREATE TABLE mapSolarSystems (
                solarSystemId INTEGER PRIMARY KEY,
                solarSystemName TEXT NOT NULL,
                constellationId INTEGER NOT NULL,
                centerX REAL, centerY REAL, centerZ REAL,
                position2DX REAL, position2DY REAL
            );
            CREATE TABLE mapSystemConnections (
                systemA INTEGER NOT NULL,
                systemB INTEGER NOT NULL,
                PRIMARY KEY (systemA, systemB)
            );
            CREATE TABLE mapPlanets (
                planetId INTEGER PRIMARY KEY,
                planetaryIndex INTEGER NOT NULL,
                solarSystemId INTEGER NOT NULL
            );
            CREATE TABLE mapMoons (
                moonId INTEGER PRIMARY KEY,
                moonIndex INTEGER NOT NULL,
                solarSystemId INTEGER NOT NULL,
                planetId INTEGER NOT NULL
            );
            CREATE TABLE mapAbstractSystems (
                solarSystemId INTEGER PRIMARY KEY,
                x REAL, y REAL,
                regionId INTEGER NOT NULL
            );

            INSERT INTO mapRegions (regionId, regionName) VALUES
                (10000001, 'Region Alpha'),
                (10000002, 'Region Beta');
            INSERT INTO mapConstellations (constellationId, constellationName, regionId, centerX, centerY, centerZ) VALUES
                (20000001, 'Const One', 10000001, 100.0, 200.0, 300.0),
                (20000002, 'Const Two', 10000002, -100.0, -200.0, -300.0);
            INSERT INTO mapSolarSystems (solarSystemId, solarSystemName, constellationId, centerX, centerY, centerZ, position2DX, position2DY) VALUES
                (30000001, 'Sys One',   20000001,  1000.0,  2000.0,  3000.0,  1000.0,  3000.0),
                (30000002, 'Sys Two',   20000001, -1000.0, -2000.0, -3000.0, -1000.0, -3000.0),
                (30000003, 'Sys Three', 20000002,  5000.0,  5000.0,  5000.0,  5000.0,  5000.0),
                (31000001, 'W-Sys',     20000002,  9000.0,  9000.0,  9000.0,  9000.0,  9000.0);
            INSERT INTO mapSystemConnections (systemA, systemB) VALUES
                (30000001, 30000002),
                (30000002, 30000003);
            INSERT INTO mapPlanets (planetId, planetaryIndex, solarSystemId) VALUES
                (40000001, 1, 30000001),
                (40000002, 2, 30000001),
                (40000003, 1, 30000003);
            INSERT INTO mapMoons (moonId, moonIndex, solarSystemId, planetId) VALUES
                (50000001, 1, 30000001, 40000001);
            INSERT INTO mapAbstractSystems (solarSystemId, x, y, regionId) VALUES
                (30000001, 10.0, 20.0, 10000001),
                (30000002, 30.0, 40.0, 10000001),
                (30000003, 50.0, 60.0, 10000002);
            ",
        )
        .expect("cannot populate fixture database");
        conn.close().expect("cannot close fixture database");
        Fixture { path }
    }

    fn manager(&self) -> SdeManager<'_> {
        SdeManager::new(&self.path, FACTOR)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

// -------------------------------------------------------------------------
// get_systempoints / get_system_connections
// -------------------------------------------------------------------------

#[test]
fn systempoints_returns_only_k_space_systems() {
    let fixture = Fixture::new("systempoints_k_space");
    let manager = fixture.manager();
    let tree = manager.get_systempoints().unwrap();
    // W-Sys (31000001) is outside the K-Space id range and must be excluded
    assert_eq!(tree.size(), 3);
    let points = map_points_to_vec(&tree);
    assert!(
        points
            .iter()
            .find(|&y| y.id == Some(31000001usize))
            .is_none()
    );
}

#[test]
fn systempoints_applies_factor_and_coordinate_inversion() {
    let fixture = Fixture::new("systempoints_factor");
    let manager = fixture.manager();
    let tree = manager.get_systempoints().unwrap();
    let points = map_points_to_vec(&tree);
    let result = points.iter().find(|point| point.id == Some(30000001usize));
    assert!(result.is_some());
    assert_eq!(result.unwrap().name, Some(String::from("Sys One")));
    // (1000, 2000, 3000) / 100 = (10, 20, 30), inverted -> (-10, -20, -30)
    // coords holds (position2DX, position2DY, 0.0)
    assert_eq!(result.unwrap().coords, [-10.0, -30.0, 0.0]);

    let result = points.iter().find(|point| point.id == Some(30000002usize));
    assert!(result.is_some());
    assert_eq!(result.unwrap().coords, [10.0, 30.0, 0.0]);
}

#[test]
fn systempoints_without_inversion_keeps_original_sign() {
    let fixture = Fixture::new("systempoints_no_invert");
    let mut manager = fixture.manager();
    manager.invert_coordinates = false;
    let tree = manager.get_systempoints().unwrap();
    let points = map_points_to_vec(&tree);
    let result = points.iter().find(|point| point.id == Some(30000001usize));
    assert!(result.is_some());
    assert_eq!(result.unwrap().coords, [10.0, 30.0, 0.0]);
}

#[test]
fn systempoints_with_negative_factor_multiplies() {
    let fixture = Fixture::new("systempoints_neg_factor");
    let mut manager = fixture.manager();
    manager.factor = -100; // negative factor multiplies by its absolute value
    let tree = manager.get_systempoints().unwrap();
    let points = map_points_to_vec(&tree);
    // (1000 * 100) inverted -> -100000
    let result = points.iter().find(|point| point.id == Some(30000001usize));
    assert!(result.is_some());
    assert_eq!(result.unwrap().coords, [-100000.0, -300000.0, 0.0]);
}

#[test]
fn system_connections_are_added_bidirectionally() {
    let fixture = Fixture::new("system_connections");
    let manager = fixture.manager();
    let tree = manager.get_systempoints().unwrap();
    assert_eq!(tree.size(), 3);
    let points = map_points_to_vec(&tree);
    let result = points.iter().find(|point| point.id == Some(30000001usize));
    assert!(result.is_some());
    assert_eq!(result.unwrap().connections.len(), 1);
    let result = points.iter().find(|point| point.id == Some(30000002usize));
    assert!(result.is_some());
    assert_eq!(result.unwrap().connections.len(), 2);
    let result = points.iter().find(|point| point.id == Some(30000003usize));
    assert!(result.is_some());
    let connection = result
        .unwrap()
        .connections
        .iter()
        .find(|&&conn| conn == (30000002, 30000003));
    assert!(connection.is_some());
}

// -------------------------------------------------------------------------
// get_connections (map lines)
// -------------------------------------------------------------------------

#[test]
fn connections_returns_lines_with_scaled_inverted_coords() {
    let fixture = Fixture::new("connections");
    let manager = fixture.manager();
    let vec_segments = manager.get_connections().unwrap();
    let expected_id = (30000001, 30000002);

    assert_eq!(vec_segments.len(), 2);
    let line = vec_segments
        .iter()
        .find(|item| item.id == expected_id)
        .expect("conn-1-2 not found");
    assert_eq!(line.id, expected_id);
    // point1 = system A (30000001): (x, y) scaled and inverted
    assert_eq!(line.point1, [-10.0, -30.0]);
    // point2 = system B (30000002)
    assert_eq!(line.point2, [10.0, 30.0]);
}

// -------------------------------------------------------------------------
// get_region
// -------------------------------------------------------------------------

#[test]
fn region_without_filters_returns_all_with_constellations() {
    let fixture = Fixture::new("region_all");
    let manager = fixture.manager();
    let regions = manager.get_region(vec![], None).unwrap();

    assert_eq!(regions.len(), 2);
    let alpha = &regions[&10000001];
    assert_eq!(alpha.name, "Region Alpha");
    assert_eq!(alpha.constellations, vec![20000001]);
    let beta = &regions[&10000002];
    assert_eq!(beta.name, "Region Beta");
    assert_eq!(beta.constellations, vec![20000002]);
}

#[test]
fn region_filtered_by_ids() {
    let fixture = Fixture::new("region_by_ids");
    let manager = fixture.manager();
    let regions = manager.get_region(vec![10000002], None).unwrap();

    assert_eq!(regions.len(), 1);
    assert!(regions.contains_key(&10000002));
}

#[test]
fn region_filtered_by_name() {
    let fixture = Fixture::new("region_by_name");
    let manager = fixture.manager();
    let regions = manager
        .get_region(vec![], Some(String::from("alpha")))
        .unwrap();

    assert_eq!(regions.len(), 1);
    assert_eq!(regions[&10000001].name, "Region Alpha");
    assert_eq!(regions[&10000001].constellations, vec![20000001]);
}

#[test]
fn region_name_filter_is_case_insensitive() {
    // SQLite's LIKE is case-insensitive for ASCII by default, so the region
    // is found even with an uppercase needle.
    let fixture = Fixture::new("region_case");
    let manager = fixture.manager();
    let regions = manager
        .get_region(vec![], Some(String::from("ALPHA")))
        .unwrap();
    assert_eq!(regions.len(), 1);
    assert!(regions.contains_key(&10000001));
}

// -------------------------------------------------------------------------
// get_universe
// -------------------------------------------------------------------------

#[test]
fn universe_with_empty_filters_returns_everything() {
    // Regression test: get_constellation()/get_solarsystem() used to always
    // bind an rarray parameter even when the filter was empty and the query
    // had no placeholder for it, so rusqlite rejected the call
    // (Error::InvalidParameterCount). Both now follow the same
    // `if filter.is_empty() { query([]) } else { query([rarray]) }` pattern
    // that `get_region` already used correctly.
    let fixture = Fixture::new("universe_empty");
    let mut manager = fixture.manager();
    assert!(manager.get_universe().is_ok());

    assert_eq!(manager.universe.regions.len(), 2);
    assert_eq!(manager.universe.constellations.len(), 2);
    // get_solarsystem has no K-space filter of its own (unlike
    // get_systempoints), so all 4 fixture systems come back, including the
    // out-of-range W-Sys.
    assert_eq!(manager.universe.solar_systems.len(), 4);

    let const_one = &manager.universe.constellations[&20000001];
    assert_eq!(const_one.name, "Const One");
    assert_eq!(const_one.region, 10000001);
    let mut systems = const_one.solar_systems.clone();
    systems.sort();
    assert_eq!(systems, vec![30000001, 30000002]);

    let const_two = &manager.universe.constellations[&20000002];
    let mut systems = const_two.solar_systems.clone();
    systems.sort();
    assert_eq!(systems, vec![30000003, 31000001]);
}

// -------------------------------------------------------------------------
// get_system_id / get_system_coords
// -------------------------------------------------------------------------

#[test]
fn system_id_searches_with_like() {
    let fixture = Fixture::new("system_id_like");
    let manager = fixture.manager();

    let results = manager.get_system_id(String::from("one")).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0],
        (
            30000001,
            String::from("Sys One"),
            10000001,
            String::from("Region Alpha")
        )
    );

    // substring search matches all four systems (including W-Sys)
    let results = manager.get_system_id(String::from("sys")).unwrap();
    assert_eq!(results.len(), 4);

    let results = manager.get_system_id(String::from("nonexistent")).unwrap();
    assert!(results.is_empty());
}

#[test]
fn system_coords_applies_factor_and_inversion() {
    let fixture = Fixture::new("system_coords");
    let manager = fixture.manager();

    let coords = manager.get_system_coords(30000001).unwrap();
    assert_eq!(coords, Some(MapPoint::new(-10.0, -20.0, -30.0)));
}

#[test]
fn system_coords_returns_none_for_unknown_id() {
    let fixture = Fixture::new("system_coords_none");
    let manager = fixture.manager();
    assert_eq!(manager.get_system_coords(30000999).unwrap(), None);
}

// -------------------------------------------------------------------------
// get_planet / get_moon
// -------------------------------------------------------------------------

#[test]
fn planet_filtered_by_solar_system() {
    let fixture = Fixture::new("planet_filtered");
    let manager = fixture.manager();
    let planets = manager.get_planet(vec![30000001]).unwrap();

    assert_eq!(planets.len(), 2);
    assert_eq!(planets[0].id, 40000001);
    assert_eq!(planets[0].index, 1);
    assert_eq!(planets[0].solar_system, 30000001);
    assert_eq!(planets[1].id, 40000002);
    assert_eq!(planets[1].index, 2);
}

#[test]
fn planet_with_empty_filter_returns_all() {
    // Regression test: get_planet() used to always bind an rarray parameter
    // even when solar_systems was empty and the query had no placeholder,
    // so rusqlite rejected the call (Error::InvalidParameterCount).
    let fixture = Fixture::new("planet_empty");
    let manager = fixture.manager();
    let planets = manager.get_planet(vec![]).unwrap();
    assert_eq!(planets.len(), 3);
}

#[test]
fn moon_with_empty_filter_returns_all() {
    // Regression test: same rarray/no-placeholder bug as get_planet() above.
    let fixture = Fixture::new("moon_empty");
    let manager = fixture.manager();
    let moons = manager.get_moon(vec![]).unwrap();
    assert_eq!(moons.len(), 1);
    assert_eq!(moons[0].id, 50000001);
}

#[test]
fn moon_filtered_by_planet_returns_matching_moons() {
    // Regression test: the query used to compare the rarray pointer against
    // a scalar (`WHERE planetId=?`), which never matched any row. It now
    // uses `WHERE planetId IN rarray(?1)`.
    let fixture = Fixture::new("moon_filtered");
    let manager = fixture.manager();
    let moons = manager.get_moon(vec![40000001]).unwrap();
    assert_eq!(moons.len(), 1);
    assert_eq!(moons[0].id, 50000001);
    assert_eq!(moons[0].planet, 40000001);
    assert_eq!(moons[0].solar_system, 30000001);

    // A planet with no moons in the fixture must return an empty result --
    // not an error, and not every moon in the table.
    let moons = manager.get_moon(vec![40000002]).unwrap();
    assert!(moons.is_empty());
}

// -------------------------------------------------------------------------
// Abstract map
// -------------------------------------------------------------------------

#[test]
fn abstract_systems_without_filter_returns_all() {
    let fixture = Fixture::new("abstract_all");
    let manager = fixture.manager();
    let tree = manager.get_abstract_systems(vec![]).unwrap();

    assert_eq!(tree.size(), 3);
    let points = map_points_to_vec(&tree);
    // coordinates are divided by the factor (no inversion on the abstract map)
    let result = points.iter().find(|point| point.id == Some(30000001usize));
    assert!(result.is_some());
    assert_eq!(result.unwrap().coords, [0.1, 0.2, 0.0]);
    let result = points.iter().find(|point| point.id == Some(30000002usize));
    assert!(result.is_some());
    assert_eq!(result.unwrap().coords, [0.3, 0.4, 0.0]);
    let result = points.iter().find(|point| point.id == Some(30000003usize));
    assert!(result.is_some());
    assert_eq!(result.unwrap().coords, [0.5, 0.6, 0.0]);
}

#[test]
fn abstract_systems_filtered_by_region() {
    let fixture = Fixture::new("abstract_by_region");
    let manager = fixture.manager();

    let tree = manager.get_abstract_systems(vec![10000001]).unwrap();
    assert_eq!(tree.size(), 2);
    let points = map_points_to_vec(&tree);
    let result = points.iter().find(|point| point.id == Some(30000001usize));
    assert!(result.is_some());
    let result = points.iter().find(|point| point.id == Some(30000002usize));
    assert!(result.is_some());
    let tree = manager.get_abstract_systems(vec![10000002]).unwrap();
    assert_eq!(tree.size(), 1);
    let points = map_points_to_vec(&tree);
    let result = points.iter().find(|point| point.id == Some(30000003usize));
    assert!(result.is_some());
}

#[test]
fn abstract_system_connections_fill_names_and_connections() {
    let fixture = Fixture::new("abstract_sys_conn");
    let manager = fixture.manager();
    let tree = manager.get_abstract_systems(vec![]).unwrap();
    let points = map_points_to_vec(&tree);

    let result = points.iter().find(|point| point.id == Some(30000001usize));
    assert!(result.is_some());
    assert_eq!(result.unwrap().name, Some(String::from("Sys One")));
    assert_eq!(result.unwrap().connections.len(), 1);
    let result = points.iter().find(|point| point.id == Some(30000002usize));
    assert!(result.is_some());
    assert_eq!(result.unwrap().connections.len(), 2);
    let result = points.iter().find(|point| point.id == Some(30000003usize));
    assert!(result.is_some());
    assert_eq!(result.unwrap().connections.len(), 1);
}

#[test]
fn abstract_system_connections_respect_region_filter() {
    let fixture = Fixture::new("abstract_sys_conn_region");
    let manager = fixture.manager();
    let tree = manager.get_abstract_systems(vec![10000001u32]).unwrap();

    assert_eq!(tree.size(), 2);
    let points = map_points_to_vec(&tree);
    // Only abstract systems inside Region Alpha are updated
    let result = points.iter().find(|point| point.id == Some(30000001usize));
    assert!(result.is_some());
    assert_eq!(result.unwrap().name, Some(String::from("Sys One")));
    let result = points.iter().find(|point| point.id == Some(30000002usize));
    assert!(result.is_some());
    assert_eq!(result.unwrap().name, Some(String::from("Sys Two")));
}

#[test]
fn abstract_connections_without_filter_returns_all_lines() {
    let fixture = Fixture::new("abstract_conn_all");
    let manager = fixture.manager();
    let vec_lines = manager.get_abstract_connections(vec![]).unwrap();
    assert_eq!(vec_lines.len(), 2);
    let found = vec_lines
        .iter()
        .find(|item| item.id == (30000001, 30000002))
        .expect("conn-1-2 not found");
    assert_eq!(found.point1, [0.1, 0.2]);
    assert_eq!(found.point2, [0.3, 0.4]);
    let found = vec_lines
        .iter()
        .find(|item| item.id == (30000002, 30000003))
        .expect("conn-2-3 not found");
    assert_eq!(found.point1, [0.3, 0.4]);
    assert_eq!(found.point2, [0.5, 0.6]);
}

#[test]
fn abstract_connections_filtered_by_region_requires_both_ends_inside() {
    let fixture = Fixture::new("abstract_conn_region");
    let manager = fixture.manager();
    let lines = manager.get_abstract_connections(vec![10000001]).unwrap();
    // conn-2-3 spans Region Alpha and Region Beta, so it is excluded
    assert_eq!(lines.len(), 1);
    let expected_id = (30000001, 30000002);
    assert!(lines.iter().any(|line| line.id == expected_id
        && line.point1 == [0.1, 0.2]
        && line.point2 == [0.3, 0.4]));
}

// -------------------------------------------------------------------------
// get_region_coordinates
// -------------------------------------------------------------------------

#[test]
fn region_coordinates_returns_bounding_box_per_region() {
    // Regression test for get_region_coordinates(): it had a typo
    // (`AX(reg.max_x)` instead of `MAX(reg.max_x)`, so SQLite rejected the
    // query outright), assigned `region.region_id` twice instead of ever
    // setting `region.name`, and read MAX()/MIN() over `REAL` columns as
    // `i64` (rusqlite doesn't coerce that; it now reads `f64` and casts).
    let fixture = Fixture::new("region_coordinates");
    let manager = fixture.manager();
    let mut areas = manager.get_region_coordinates().unwrap();
    areas.sort_by_key(|area| area.region_id);

    assert_eq!(areas.len(), 2);

    let alpha = &areas[0];
    assert_eq!(alpha.region_id, 10000001);
    assert_eq!(alpha.name, "Region Alpha");
    // Region Alpha's fixture systems are position2D (1000,3000) and
    // (-1000,-3000); coordinate inversion (swap + negate) maps this
    // symmetric bounding box back onto itself. Z is always 0 -- the
    // bounding box has been 2D since the migration from projX/Y/Z to position2DX/Y.
    assert_eq!(alpha.max, MapPoint::new(1000.0, 3000.0, 0.0));
    assert_eq!(alpha.min, MapPoint::new(-1000.0, -3000.0, 0.0));

    let beta = &areas[1];
    assert_eq!(beta.region_id, 10000002);
    assert_eq!(beta.name, "Region Beta");
    // Region Beta's fixture systems are position2D (5000,5000) and
    // (9000,9000); after inversion new_max = -old_min, new_min = -old_max.
    assert_eq!(beta.max, MapPoint::new(-5000.0, -5000.0, 0.0));
    assert_eq!(beta.min, MapPoint::new(-9000.0, -9000.0, 0.0));
}
