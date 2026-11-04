//! `SdeManager` reading a database built by the real parser.
//!
//! `tests/manager.rs` reads a fixture whose tables are written by hand, so a
//! column renamed in `schema.sql` would go unnoticed there. Here the database
//! is created with `create_schema` and filled by `Parser::parse_data` from a
//! small export, so every query of the read path runs against the schema the
//! crate really ships.
//!
//! The export has two solar systems joined by a stargate: Jita, which carries
//! CCP's 2D position, and Perimeter, which has none.
#![cfg(feature = "builder")]

use rusqlite::Connection;
use sde::SdeManager;
use sde::builder::parser::{Parser, ParserConfig};
use sde::builder::schema::create_schema;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// A temporary export directory and the database parsed from it, both removed
/// on drop.
struct ParsedFixture {
    directory: PathBuf,
    database: PathBuf,
}

impl ParsedFixture {
    fn new(test_name: &str) -> Self {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!(
            "sde_parsed_{}_{}_{}",
            test_name,
            std::process::id(),
            id
        ));
        let directory = base.join("export");
        std::fs::create_dir_all(&directory).expect("cannot create the export directory");
        for (name, content) in Self::export() {
            std::fs::write(directory.join(name), content).expect("cannot write an export file");
        }

        let database = base.join("sde.db");
        let mut connection = Connection::open(&database).expect("cannot create the database");
        create_schema(&connection).expect("cannot create the schema");
        Parser::new(&directory, ParserConfig::default())
            .parse_data(&mut connection)
            .expect("the export should parse");
        connection.close().expect("cannot close the database");

        Self {
            directory: base,
            database,
        }
    }

    fn manager(&self) -> SdeManager<'_> {
        SdeManager::new(&self.database, 1.0).unwrap()
    }

    /// The files of the export: (file name, content).
    fn export() -> Vec<(&'static str, &'static str)> {
        vec![
            (
                "categories.jsonl",
                r#"{"_key": 6, "name": {"en": "Celestial"}, "published": true}"#,
            ),
            (
                "groups.jsonl",
                r#"{"_key": 6, "categoryID": 6, "name": {"en": "Sun"}, "anchorable": false}
{"_key": 7, "categoryID": 6, "name": {"en": "Frigate"}, "anchorable": false}"#,
            ),
            ("races.jsonl", r#"{"_key": 1, "name": {"en": "Caldari"}}"#),
            (
                "npcCorporations.jsonl",
                r#"{"_key": 1000004, "name": {"en": "CBD Corporation"}, "tickerName": "CBD", "deleted": false, "extent": "L", "hasPlayerPersonnelManager": false, "initialPrice": 0, "memberLimit": -1, "minSecurity": 0.0, "minimumJoinStanding": 1, "sendCharTerminationMessage": true, "shares": 1000, "size": "L", "taxRate": 0.0, "uniqueName": true, "iconID": 500, "raceID": 1}"#,
            ),
            (
                "factions.jsonl",
                r#"{"_key": 500001, "name": {"en": "Caldari State"}, "iconID": 600, "sizeFactor": 3.0, "uniqueName": true, "description": {"en": "x"}, "corporationID": 1000004, "memberRaces": [1]}"#,
            ),
            ("npcCorporationDivisions.jsonl", ""),
            ("stationServices.jsonl", ""),
            ("stationOperations.jsonl", ""),
            ("npcStations.jsonl", ""),
            (
                "mapRegions.jsonl",
                r#"{"_key": 10000002, "name": {"en": "The Forge"}, "nebulaID": 5, "position": {"x": 100.0, "y": 200.0, "z": 300.0}}"#,
            ),
            (
                "mapConstellations.jsonl",
                r#"{"_key": 20000020, "name": {"en": "Kimotoro"}, "regionID": 10000002, "position": {"x": 110.0, "y": 210.0, "z": 310.0}}"#,
            ),
            (
                "mapSolarSystems.jsonl",
                r#"{"_key": 30000142, "name": {"en": "Jita"}, "constellationID": 20000020, "radius": 999999999.0, "position": {"x": -100.0, "y": 200.0, "z": -300.0}, "securityStatus": 0.9459, "securityClass": "B", "corridor": false, "fringe": false, "hub": true, "international": true, "regional": true, "luminosity": 0.049, "position2D": {"x": 12.5, "y": -7.25}}
{"_key": 30002187, "name": {"en": "Perimeter"}, "constellationID": 20000020, "radius": 1.0, "position": {"x": 0.0, "y": 0.0, "z": 0.0}, "securityStatus": -0.35}"#,
            ),
            (
                "mapStargates.jsonl",
                r#"{"_key": 50000001, "solarSystemID": 30000142, "typeID": 16, "position": {"x": 1.0, "y": 2.0, "z": 3.0}, "destination": {"stargateID": 50000002, "solarSystemID": 30002187}}
{"_key": 50000002, "solarSystemID": 30002187, "typeID": 16, "position": {"x": 4.0, "y": 5.0, "z": 6.0}, "destination": {"stargateID": 50000001, "solarSystemID": 30000142}}"#,
            ),
            (
                "mapStars.jsonl",
                r#"{"_key": 40000001, "radius": 63350000, "solarSystemID": 30000142, "statistics": {"age": 4.5e17, "life": 6.9e17, "luminosity": 0.01575, "spectralClass": "K2 V", "temperature": 4567.0}, "typeID": 3000}"#,
            ),
            (
                "mapPlanets.jsonl",
                r#"{"_key": 40000002, "celestialIndex": 1, "position": {"x": 161891117336.0, "y": 21288951986.0, "z": -73529712226.0}, "radius": 5060000, "solarSystemID": 30000142, "statistics": {"locked": false}, "typeID": 11}"#,
            ),
            (
                "mapMoons.jsonl",
                r#"{"_key": 40000004, "solarSystemID": 30000142, "orbitIndex": 1, "orbitID": 40000002, "typeID": 12, "radius": 100000, "position": {"x": 1.0, "y": 2.0, "z": 3.0}}"#,
            ),
            (
                "types.jsonl",
                r#"{"_key": 3000, "groupID": 6, "name": {"en": "Yellow G5 (ffcc00)"}, "iconID": 100, "published": true, "volume": 0.0}
{"_key": 16, "groupID": 7, "name": {"en": "Stargate"}, "published": true}
{"_key": 11, "groupID": 7, "name": {"en": "Planet (Barren)"}, "published": true}
{"_key": 12, "groupID": 7, "name": {"en": "Moon"}, "published": true}"#,
            ),
        ]
    }
}

impl Drop for ParsedFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

#[test]
fn the_universe_carries_the_security_the_parser_stored() {
    let fixture = ParsedFixture::new("universe_security");
    let mut manager = fixture.manager();
    manager.get_universe().unwrap();

    let jita = &manager.universe.solar_systems[&30000142];
    assert_eq!(jita.name, "Jita");
    assert_eq!(jita.region, 10000002);
    assert_eq!(jita.constellation, 20000020);
    assert_eq!(jita.security, 0.9459);

    // A negative value survives the round trip too.
    let perimeter = &manager.universe.solar_systems[&30002187];
    assert_eq!(perimeter.security, -0.35);
}

#[test]
fn the_universe_reads_the_star_the_planet_and_the_moon() {
    let fixture = ParsedFixture::new("universe_bodies");
    let mut manager = fixture.manager();
    manager.get_universe().unwrap();

    let jita = &manager.universe.solar_systems[&30000142];
    let star = jita.star.as_ref().expect("Jita should have a star");
    assert_eq!(star.id, 40000001);
    assert!(manager.universe.solar_systems[&30002187].star.is_none());

    assert_eq!(manager.get_planet(Vec::new()).unwrap().len(), 1);
    assert_eq!(manager.get_moon(Vec::new()).unwrap().len(), 1);
}

#[test]
fn a_system_without_a_2d_position_is_in_the_universe_but_not_on_the_map() {
    let fixture = ParsedFixture::new("no_position_2d");
    let mut manager = fixture.manager();

    // Perimeter has no position2D in the export: the map leaves it out.
    let points = manager.get_systems().unwrap();
    assert_eq!(points.len(), 1);
    let jita = &points[&30000142usize];
    assert_eq!(jita.coords, [12.5, 7.25, 0.0]);
    assert_eq!(jita.connections, vec![(30000142, 30002187)]);

    // The universe keeps it, with the projection at the origin.
    manager.get_universe().unwrap();
    let perimeter = &manager.universe.solar_systems[&30002187];
    assert_eq!(perimeter.projected_coords.coords, [0.0, 0.0, 0.0]);
}

#[test]
fn names_regions_and_types_read_the_shipped_schema() {
    let fixture = ParsedFixture::new("names");
    let manager = fixture.manager();

    let names = manager.get_system_names(Vec::new()).unwrap();
    assert_eq!(names.len(), 2);
    assert_eq!(names[&30002187], "Perimeter");

    let regions = manager.get_system_regions().unwrap();
    assert_eq!(regions[&30000142], 10000002);
    assert_eq!(regions[&30002187], 10000002);

    let types = manager.get_type_names(vec![16, 12]).unwrap();
    assert_eq!(types[&16], "Stargate");
    assert_eq!(types[&12], "Moon");

    // Category 6 holds the two groups of the export.
    assert_eq!(manager.get_category_types(6, true).unwrap().len(), 4);
}
