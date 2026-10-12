//! Integration tests for `GateGraph`: loading from `mapSystemGates` through
//! `SdeManager`, and (when a real `sde.db` is next to the crate) routing
//! over the whole universe.

use rusqlite::Connection;
use sde::SdeManager;
use sde::gate_graph::{Endpoint, GateGraph, Priority};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// A temporary database with only `mapSystemGates`, removed on drop.
struct Fixture {
    path: PathBuf,
}

impl Fixture {
    fn new(test_name: &str) -> Self {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "sde_gate_graph_{}_{}_{}.db",
            std::process::id(),
            id,
            test_name
        ));
        let _ = std::fs::remove_file(&path);
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE mapSystemGates (
                    systemGateId INTEGER NOT NULL,
                    solarSystemId INTEGER NOT NULL,
                    destinationGateId INTEGER NOT NULL,
                    destinationSystemId INTEGER NOT NULL,
                    typeId INTEGER NOT NULL,
                    positionX REAL NOT NULL, positionY REAL NOT NULL, positionZ REAL NOT NULL
                );
                INSERT INTO mapSystemGates VALUES
                    (11, 1, 21, 2, 1, 0.0, 0.0, 0.0),
                    (21, 2, 11, 1, 1, 0.0, 0.0, 0.0),
                    (22, 2, 31, 3, 1, 300.0, 400.0, 0.0),
                    (31, 3, 22, 2, 1, 0.0, 0.0, 0.0);",
            )
            .unwrap();
        Fixture { path }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[test]
fn loads_the_gates_and_routes_over_them() {
    let fixture = Fixture::new("load");
    let manager = SdeManager::new(&fixture.path, 1.0).unwrap();
    let graph = GateGraph::load(&manager).unwrap();
    assert_eq!(graph.gate_count(), 4);
    assert_eq!(graph.system_count(), 3);

    let route = graph
        .route(
            Endpoint::Gate(11),
            Endpoint::System(3),
            &Priority::balanced(10.0, 0.0),
        )
        .unwrap()
        .unwrap();
    assert_eq!(route.gates, vec![11, 21, 22, 31]);
    assert_eq!(route.jumps, 2);
    // gates 21 and 22 are 300 and 400 apart on two axes: a 3-4-5 triangle
    assert_eq!(route.distance, 500.0);
    assert_eq!(route.cost, 520.0);
}

#[test]
fn the_scaling_factor_does_not_change_distances() {
    let fixture = Fixture::new("factor");
    let scaled = SdeManager::new(&fixture.path, 1000.0).unwrap();
    let raw = SdeManager::new(&fixture.path, 1.0).unwrap();
    let cost = Priority::balanced(0.0, 0.0);
    let a = GateGraph::load(&scaled)
        .unwrap()
        .route(Endpoint::Gate(11), Endpoint::Gate(31), &cost)
        .unwrap()
        .unwrap();
    let b = GateGraph::load(&raw)
        .unwrap()
        .route(Endpoint::Gate(11), Endpoint::Gate(31), &cost)
        .unwrap()
        .unwrap();
    assert_eq!(a, b);
}

#[test]
fn a_database_without_gates_is_an_error() {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let path = std::env::temp_dir().join(format!(
        "sde_gate_graph_empty_{}_{}.db",
        std::process::id(),
        id
    ));
    let _ = std::fs::remove_file(&path);
    Connection::open(&path)
        .unwrap()
        .execute_batch("CREATE TABLE unrelated (a INTEGER);")
        .unwrap();
    let manager = SdeManager::new(&path, 1.0).unwrap();
    assert!(GateGraph::load(&manager).is_err());
    let _ = std::fs::remove_file(&path);
}

/// Routes over the real database. It is not part of the repository
/// (`sde.db` is ignored by git), so the test does nothing when the file is
/// missing.
#[test]
fn routes_over_the_real_universe() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("sde.db");
    if !path.exists() {
        eprintln!("sde.db not found, skipping");
        return;
    }
    let manager = SdeManager::new(&path, 1.0).unwrap();
    let graph = GateGraph::load(&manager).unwrap();
    assert!(graph.gate_count() > 10_000);

    // Jita -> Amarr
    let (jita, amarr) = (30000142, 30002187);
    let few_jumps = graph
        .route(
            Endpoint::System(jita),
            Endpoint::System(amarr),
            &Priority::FewestJumps,
        )
        .unwrap()
        .unwrap();
    let short_flights = graph
        .route(
            Endpoint::System(jita),
            Endpoint::System(amarr),
            &Priority::ShortestFlight,
        )
        .unwrap()
        .unwrap();

    assert!(few_jumps.jumps > 0);
    // Making jumps expensive can only reduce them, and making them free
    // can only reduce the distance flown.
    assert!(few_jumps.jumps <= short_flights.jumps);
    assert!(short_flights.distance <= few_jumps.distance);
    // The two ends of every jump are gates facing each other.
    for pair in few_jumps.gates.windows(2) {
        let (a, b) = (graph.gate(pair[0]).unwrap(), graph.gate(pair[1]).unwrap());
        if a.system_id != b.system_id {
            assert_eq!(a.destination_id, b.id);
        }
    }
}
