//! Moving apart the systems the 2D projection puts on top of each other
//! (feature `projection-patch`, enabled by default).
//!
//! `mapSolarSystems.position2DX`/`position2DY` come from a projection, and a
//! projection lays some systems almost on top of each other: a map that draws
//! every system as a node can't tell them apart. [`apply`] is a
//! postprocessing step for the finished database, independent of how it was
//! built: it pushes apart every pair of systems whose centers are closer than
//! [`ProjectionPatch::distance`] until none is, and writes the new positions
//! back.
//!
//! - It only reads and writes `position2DX`/`position2DY`. The regions'
//!   areas and everything else that is derived from them are left to
//!   whoever reads them.
//! - It is deterministic: the same database and the same settings always
//!   give the same positions.
//! - It is idempotent, and leaves nothing behind to say it ran: on a
//!   database that is already patched no two systems are too close, so it
//!   moves nothing and writes nothing ([`PatchOutcome::moved`] is 0). That
//!   makes it safe to apply after every build or update, and on a database
//!   that was built before it existed. A database that is rebuilt has its
//!   original positions back, and the next run patches them again.
//! - The original positions are not kept: nothing else reads them.
//!
//! The distance is in the units of the map the database is read at: the
//! columns hold very large numbers, and [`crate::SdeManager`] divides them by
//! a `factor`. [`ProjectionPatch::factor`] is that same factor, so a
//! distance of 16 means 16 map units for whoever reads with it.
//!
//! ```no_run
//! # fn demo() -> Result<(), sde::Error> {
//! use sde::projection_patch::{self, ProjectionPatch};
//!
//! let patch = ProjectionPatch {
//!     factor: 50_000_000_000_000.0,
//!     ..ProjectionPatch::default()
//! };
//! let outcome = projection_patch::apply("sde.db".as_ref(), &patch)?;
//! println!("{outcome}");
//! # Ok(())
//! # }
//! ```

use crate::Error;
use rusqlite::{Connection, params};
use std::collections::HashMap;
use std::fmt;
use std::ops::RangeInclusive;
use std::path::Path;

/// How the systems are moved apart.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectionPatch {
    /// The closest two system centers may be after the patch, in map units
    /// (see [`Self::factor`]). Default `16.0`: with nodes of radius 4, a gap
    /// of 8 between them.
    pub distance: f64,
    /// The factor the database is read with ([`crate::SdeManager::new`]):
    /// the columns are divided by it when it is greater than 1, and
    /// multiplied by its absolute value when it is less than -1. Any other
    /// value leaves them as they are. Default `50_000_000_000_000.0`.
    pub factor: f64,
    /// The systems that are moved, by `solarSystemId`. Default: the K-space
    /// ones, which are the ones [`crate::SdeManager::get_systems`] reads.
    pub systems: RangeInclusive<i64>,
    /// How many rounds of pushing apart are tried before giving up.
    /// Default `500`.
    pub max_iterations: u32,
}

impl Default for ProjectionPatch {
    fn default() -> Self {
        Self {
            distance: 16.0,
            factor: 50_000_000_000_000.0,
            systems: 30_000_000..=30_999_999,
            max_iterations: 500,
        }
    }
}

impl ProjectionPatch {
    /// [`Self::distance`] in the units of the columns: the inverse of what
    /// [`crate::SdeManager`] does when it reads them.
    fn raw_distance(&self) -> f64 {
        if self.factor > 1.0 {
            self.distance * self.factor
        } else if self.factor < -1.0 {
            self.distance / self.factor.abs()
        } else {
            self.distance
        }
    }
}

/// What [`apply`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PatchOutcome {
    /// How many systems changed their position: none when no two were
    /// closer than the distance (a database that is already patched).
    pub moved: usize,
}

impl fmt::Display for PatchOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.moved {
            0 => write!(f, "projection patch: no system is too close, nothing moved"),
            moved => write!(f, "projection patch moved {moved} systems apart"),
        }
    }
}

/// Applies `patch` to the database at `db`. See the [module](self) docs.
///
/// The new positions are written in one transaction: a failure leaves the
/// database as it was.
#[tracing::instrument]
pub fn apply(db: &Path, patch: &ProjectionPatch) -> Result<PatchOutcome, Error> {
    let mut connection = Connection::open(db)?;
    apply_to(&mut connection, patch)
}

/// [`apply`] on a connection that is already open.
pub fn apply_to(
    connection: &mut Connection,
    patch: &ProjectionPatch,
) -> Result<PatchOutcome, Error> {
    let distance = patch.raw_distance();
    if !distance.is_finite() || distance <= 0.0 {
        return Err(Error::projection_patch(format!(
            "the distance must be a positive number (got {} with factor {})",
            patch.distance, patch.factor
        )));
    }
    let first = *patch.systems.start();
    let last = *patch.systems.end();

    let transaction = connection.transaction()?;
    let mut ids: Vec<i64> = Vec::new();
    let mut original: Vec<[f64; 2]> = Vec::new();
    {
        let mut statement = transaction.prepare(
            "SELECT solarSystemId, position2DX, position2DY FROM mapSolarSystems \
             WHERE solarSystemId BETWEEN ?1 AND ?2 \
             AND position2DX IS NOT NULL AND position2DY IS NOT NULL \
             ORDER BY solarSystemId",
        )?;
        let mut rows = statement.query(params![first, last])?;
        while let Some(row) = rows.next()? {
            ids.push(row.get(0)?);
            original.push([row.get(1)?, row.get(2)?]);
        }
    }

    let mut positions = original.clone();
    spread(&mut positions, distance, patch.max_iterations)?;

    let mut moved = 0;
    {
        let mut update = transaction.prepare(
            "UPDATE mapSolarSystems SET position2DX = ?1, position2DY = ?2 \
             WHERE solarSystemId = ?3",
        )?;
        for ((id, before), after) in ids.iter().zip(&original).zip(&positions) {
            if before != after {
                update.execute(params![after[0], after[1], id])?;
                moved += 1;
            }
        }
    }
    transaction.commit()?;
    Ok(PatchOutcome { moved })
}

/// Pushes the points apart, in place, until every pair is at least `distance`
/// from each other. Each round, every pair that is too close moves half of
/// what is missing each, along the line between them, so that a point that
/// is pushed into a third one is dealt with by the next round. The points
/// are visited in order, so the result doesn't depend on anything but the
/// input.
fn spread(points: &mut [[f64; 2]], distance: f64, max_iterations: u32) -> Result<(), Error> {
    // A margin, so that a pair that has just been separated is not closer
    // than `distance` again by rounding, and the next round has nothing to do.
    let margin = distance * 1e-6;
    let limit = distance * (1.0 - 1e-9);
    // One more look after the last round of pushing: it may have been enough.
    for round in 0..=max_iterations {
        let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        for (index, point) in points.iter().enumerate() {
            grid.entry(cell(point, distance)).or_default().push(index);
        }
        let mut push = vec![[0.0_f64; 2]; points.len()];
        let mut crowded = false;
        for (index, point) in points.iter().enumerate() {
            let (cell_x, cell_y) = cell(point, distance);
            for around_x in -1..=1 {
                for around_y in -1..=1 {
                    let Some(others) = grid.get(&(cell_x + around_x, cell_y + around_y)) else {
                        continue;
                    };
                    for &other in others.iter().filter(|&&other| other > index) {
                        let delta_x = points[other][0] - point[0];
                        let delta_y = points[other][1] - point[1];
                        let apart = delta_x.hypot(delta_y);
                        if apart >= limit {
                            continue;
                        }
                        crowded = true;
                        // Two points in the very same place have no
                        // direction between them: the index gives one, always
                        // the same.
                        let (unit_x, unit_y) = if apart > 1e-9 * distance {
                            (delta_x / apart, delta_y / apart)
                        } else {
                            let angle = (index as u64).wrapping_mul(2_654_435_761) % 1000;
                            let angle = angle as f64 / 1000.0 * std::f64::consts::TAU;
                            (angle.cos(), angle.sin())
                        };
                        let half = (distance - apart) / 2.0 + margin;
                        push[index][0] -= unit_x * half;
                        push[index][1] -= unit_y * half;
                        push[other][0] += unit_x * half;
                        push[other][1] += unit_y * half;
                    }
                }
            }
        }
        if !crowded {
            return Ok(());
        }
        if round == max_iterations {
            break;
        }
        for (point, push) in points.iter_mut().zip(&push) {
            point[0] += push[0];
            point[1] += push[1];
        }
    }
    Err(Error::projection_patch(format!(
        "{max_iterations} rounds were not enough to keep every system {distance} apart"
    )))
}

/// The cell of the grid (of side `size`) a point is in.
fn cell(point: &[f64; 2], size: f64) -> (i64, i64) {
    (
        (point[0] / size).floor() as i64,
        (point[1] / size).floor() as i64,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A database with just the table and columns the patch touches.
    fn database(systems: &[(i64, Option<f64>, Option<f64>)]) -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE mapSolarSystems (
                   solarSystemId INTEGER PRIMARY KEY,
                   position2DX REAL,
                   position2DY REAL
                 ) STRICT;",
            )
            .unwrap();
        for (id, x, y) in systems {
            connection
                .execute(
                    "INSERT INTO mapSolarSystems VALUES (?1, ?2, ?3)",
                    params![id, x, y],
                )
                .unwrap();
        }
        connection
    }

    fn positions(connection: &Connection) -> Vec<(i64, f64, f64)> {
        let mut statement = connection
            .prepare(
                "SELECT solarSystemId, position2DX, position2DY FROM mapSolarSystems \
                 WHERE position2DX IS NOT NULL ORDER BY solarSystemId",
            )
            .unwrap();
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    fn apart(a: (i64, f64, f64), b: (i64, f64, f64)) -> f64 {
        (a.1 - b.1).hypot(a.2 - b.2)
    }

    /// A patch that reads the columns as they are, so a distance is in the
    /// same units as the test data.
    fn plain(distance: f64) -> ProjectionPatch {
        ProjectionPatch {
            distance,
            factor: 1.0,
            ..ProjectionPatch::default()
        }
    }

    const A: i64 = 30_000_001;
    const B: i64 = 30_000_002;
    const C: i64 = 30_000_003;

    #[test]
    fn the_defaults_are_a_distance_of_16_for_the_k_space_systems() {
        let patch = ProjectionPatch::default();
        assert_eq!(patch.distance, 16.0);
        assert_eq!(patch.systems, 30_000_000..=30_999_999);
    }

    #[test]
    fn two_close_systems_are_moved_apart_along_their_line_and_nothing_else_moves() {
        let mut connection = database(&[
            (A, Some(100.0), Some(100.0)),
            (B, Some(104.0), Some(100.0)),
            (C, Some(500.0), Some(500.0)),
        ]);
        let outcome = apply_to(&mut connection, &plain(16.0)).unwrap();
        assert_eq!(outcome, PatchOutcome { moved: 2 });
        let after = positions(&connection);
        assert!(apart(after[0], after[1]) >= 16.0);
        // Along the line between them, symmetrically around their middle.
        assert!((after[0].2 - 100.0).abs() < 1e-9 && (after[1].2 - 100.0).abs() < 1e-9);
        assert!(((after[0].1 + after[1].1) / 2.0 - 102.0).abs() < 1e-6);
        // The far one is untouched.
        assert_eq!((after[2].1, after[2].2), (500.0, 500.0));
    }

    #[test]
    fn a_push_into_a_third_system_is_settled_by_the_next_rounds() {
        // Three in a row, 10 apart: separating the first two pushes the
        // middle one into the third.
        let mut connection = database(&[
            (A, Some(0.0), Some(0.0)),
            (B, Some(10.0), Some(0.0)),
            (C, Some(20.0), Some(0.0)),
        ]);
        apply_to(&mut connection, &plain(16.0)).unwrap();
        let after = positions(&connection);
        for (first, second) in [(0, 1), (0, 2), (1, 2)] {
            assert!(apart(after[first], after[second]) >= 16.0 - 1e-6);
        }
    }

    #[test]
    fn systems_in_the_very_same_place_are_separated() {
        let mut connection = database(&[(A, Some(7.0), Some(7.0)), (B, Some(7.0), Some(7.0))]);
        apply_to(&mut connection, &plain(16.0)).unwrap();
        let after = positions(&connection);
        assert!(apart(after[0], after[1]) >= 16.0 - 1e-6);
    }

    #[test]
    fn applying_it_again_moves_nothing_and_leaves_no_trace_of_having_run() {
        let mut connection = database(&[(A, Some(0.0), Some(0.0)), (B, Some(3.0), Some(0.0))]);
        apply_to(&mut connection, &plain(16.0)).unwrap();
        let once = positions(&connection);
        let outcome = apply_to(&mut connection, &plain(16.0)).unwrap();
        assert_eq!(outcome, PatchOutcome { moved: 0 });
        assert_eq!(positions(&connection), once);
        // It only ever touches the two columns: no table of its own.
        let tables: i64 = connection
            .query_row("SELECT COUNT(*) FROM sqlite_master", [], |row| row.get(0))
            .unwrap();
        assert_eq!(tables, 1);
    }

    #[test]
    fn a_larger_distance_is_applied_on_top_of_the_patched_positions() {
        let mut connection = database(&[(A, Some(0.0), Some(0.0)), (B, Some(3.0), Some(0.0))]);
        apply_to(&mut connection, &plain(16.0)).unwrap();
        let outcome = apply_to(&mut connection, &plain(30.0)).unwrap();
        assert_eq!(outcome, PatchOutcome { moved: 2 });
        let after = positions(&connection);
        assert!(apart(after[0], after[1]) >= 30.0 - 1e-6);
    }

    #[test]
    fn a_database_with_nothing_too_close_is_left_as_it_is() {
        let mut connection = database(&[(A, Some(0.0), Some(0.0)), (B, Some(100.0), Some(0.0))]);
        let outcome = apply_to(&mut connection, &plain(16.0)).unwrap();
        assert_eq!(outcome, PatchOutcome { moved: 0 });
        let after = positions(&connection);
        assert_eq!((after[0].1, after[1].1), (0.0, 100.0));
    }

    #[test]
    fn the_distance_is_in_the_units_the_factor_reads_the_columns_at() {
        // Read at a factor of 100 (the columns are divided by it), a distance
        // of 16 is 1600 in the columns: 1000 apart is too close.
        let mut connection = database(&[(A, Some(0.0), Some(0.0)), (B, Some(1000.0), Some(0.0))]);
        let patch = ProjectionPatch {
            distance: 16.0,
            factor: 100.0,
            ..ProjectionPatch::default()
        };
        apply_to(&mut connection, &patch).unwrap();
        let after = positions(&connection);
        assert!(apart(after[0], after[1]) >= 1600.0 - 1e-3);

        // With a factor below -1 the columns are multiplied: 16 map units
        // are 0.16 in the columns.
        let mut connection = database(&[(A, Some(0.0), Some(0.0)), (B, Some(0.1), Some(0.0))]);
        let patch = ProjectionPatch {
            distance: 16.0,
            factor: -100.0,
            ..ProjectionPatch::default()
        };
        apply_to(&mut connection, &patch).unwrap();
        let after = positions(&connection);
        assert!(apart(after[0], after[1]) >= 0.16 - 1e-9);
    }

    #[test]
    fn systems_outside_the_range_or_without_a_position_are_left_alone() {
        let mut connection = database(&[
            (A, Some(0.0), Some(0.0)),
            // A wormhole system on top of the first one.
            (31_000_001, Some(1.0), Some(0.0)),
            // No projection at all.
            (B, None, None),
        ]);
        let outcome = apply_to(&mut connection, &plain(16.0)).unwrap();
        assert_eq!(outcome, PatchOutcome { moved: 0 });
        let wormhole: (f64, f64) = connection
            .query_row(
                "SELECT position2DX, position2DY FROM mapSolarSystems WHERE solarSystemId = ?1",
                [31_000_001],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(wormhole, (1.0, 0.0));
    }

    #[test]
    fn the_same_input_always_gives_the_same_positions() {
        let systems: Vec<_> = (0..40)
            .map(|index| {
                (
                    30_000_000 + index,
                    Some((index % 7) as f64 * 3.0),
                    Some((index / 7) as f64 * 2.0),
                )
            })
            .collect();
        let mut first = database(&systems);
        let mut second = database(&systems);
        apply_to(&mut first, &plain(16.0)).unwrap();
        apply_to(&mut second, &plain(16.0)).unwrap();
        assert_eq!(positions(&first), positions(&second));
        let after = positions(&first);
        for (index, a) in after.iter().enumerate() {
            for b in &after[index + 1..] {
                assert!(apart(*a, *b) >= 16.0 - 1e-6);
            }
        }
    }

    #[test]
    fn a_distance_that_is_not_positive_is_an_error() {
        let mut connection = database(&[(A, Some(0.0), Some(0.0))]);
        for distance in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(apply_to(&mut connection, &plain(distance)).is_err());
        }
    }

    #[test]
    fn running_out_of_rounds_is_an_error_and_leaves_the_database_as_it_was() {
        let mut connection = database(&[(A, Some(0.0), Some(0.0)), (B, Some(1.0), Some(0.0))]);
        let patch = ProjectionPatch {
            max_iterations: 0,
            ..plain(16.0)
        };
        assert!(apply_to(&mut connection, &patch).is_err());
        let after = positions(&connection);
        assert_eq!((after[0].1, after[1].1), (0.0, 1.0));
        // The next try with enough rounds does the work.
        let outcome = apply_to(&mut connection, &plain(16.0)).unwrap();
        assert_eq!(outcome, PatchOutcome { moved: 2 });
    }
}
