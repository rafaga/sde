//! Creation of `sde.db`'s STRICT schema.
//!
//! Equivalent to `SdeParser.create_table_structure()` in the Python
//! prototype (`sde_parser.py`), which opens `schema_corregido_strict.sql`
//! from disk and runs it against the connection with
//! `cur.executescript(query)`.
//!
//! Here the DDL is embedded into the binary at compile time with
//! `include_str!` instead of being read from the filesystem at runtime:
//! `sde`'s builder doesn't depend on an external `.sql` file existing at
//! a given relative path (fragile if the binary runs from a different
//! working directory), and the DDL stays versioned alongside the code
//! that consumes it.
//!
//! `schema.sql` is a 1:1 copy of `schema_corregido_strict.sql` from the
//! `databaseCreator` project (the schema's original source of truth);
//! if that file changes, this copy needs to be updated by hand.
//!
//! Note on "final write to sqlite": this module only covers creating the
//! empty schema (DDL). Populating it with real SDE data (the equivalent
//! of `sde_parser.py`'s ~15 `_parse_*` methods: categories, groups,
//! types, races, factions, regions, constellations, solar systems with
//! isometric/dimetric projection, stargates, stars, planets, moons and
//! connections) is a considerably bigger piece of work and lives outside
//! this module -- see `builder::mod` for the rest of the builder's
//! pieces.

use rusqlite::Connection;

/// The complete STRICT DDL (tables, indexes, FKs) for `sde.db`.
///
/// Kept manually in sync with `schema_corregido_strict.sql` from the
/// `databaseCreator` project -- see that file for the change history and
/// notes on the September 2025 SDE rework.
pub const SCHEMA_DDL: &str = include_str!("schema.sql");

/// Creates the full STRICT schema on `connection`, running
/// [`SCHEMA_DDL`] in one go.
///
/// The DDL's first statement is `PRAGMA foreign_keys = ON;`, so after
/// this call the connection has FK enforcement active for the rest of
/// its life (unless something else explicitly disables it) -- there's
/// no need to set that pragma again before inserting data on the same
/// `connection`.
///
/// The DDL deliberately doesn't use `CREATE TABLE IF NOT EXISTS`:
/// calling this function twice on the same connection fails (`table X
/// already exists`) instead of silently mixing in with a partial or
/// corrupted schema from a previous run. The caller is responsible for
/// passing in a "clean" connection (a new or empty database).
pub fn create_schema(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(SCHEMA_DDL)
}

/// Names of the tables declared by [`SCHEMA_DDL`], in the order they
/// appear.
///
/// Deliberately simple parsing (one pass per line looking for the
/// `CREATE TABLE ` prefix): the DDL is a static, trusted resource owned
/// by the crate itself, not external input, so it's not worth pulling in
/// a real SQL parser just for this. Useful for post-creation checks
/// (e.g. in tests) without keeping a separate manual list that could
/// drift out of sync with the real DDL.
pub fn table_names() -> Vec<&'static str> {
    SCHEMA_DDL
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix("CREATE TABLE "))
        .map(|rest| rest.split_whitespace().next().unwrap_or(""))
        .filter(|name| !name.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_schema_succeeds_on_fresh_connection() {
        let connection = Connection::open_in_memory().unwrap();
        create_schema(&connection).unwrap();
    }

    #[test]
    fn create_schema_creates_every_declared_table() {
        let connection = Connection::open_in_memory().unwrap();
        create_schema(&connection).unwrap();

        let mut statement = connection
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .unwrap();
        let mut existing: Vec<String> = statement
            .query_map([], |row| row.get::<usize, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        existing.sort();

        let mut expected: Vec<String> = table_names().iter().map(|s| s.to_string()).collect();
        expected.sort();

        assert_eq!(existing, expected);
        // Extra anchor: if this number changes, the DDL likely changed
        // too, and it's worth reviewing the rest of this test file.
        assert_eq!(expected.len(), 22);
    }

    #[test]
    fn create_schema_fails_if_called_twice() {
        // The DDL deliberately doesn't use `CREATE TABLE IF NOT EXISTS`:
        // a second call on the same connection must fail visibly, not
        // silently mix in with a previous schema.
        let connection = Connection::open_in_memory().unwrap();
        create_schema(&connection).unwrap();
        assert!(create_schema(&connection).is_err());
    }

    #[test]
    fn create_schema_enforces_strict_typing() {
        let connection = Connection::open_in_memory().unwrap();
        create_schema(&connection).unwrap();

        // invCategories.published is `INTEGER NOT NULL CHECK (published
        // IN (0,1))`; in a STRICT table, inserting TEXT into an INTEGER
        // column must fail outright instead of being silently coerced by
        // affinity (standard, non-STRICT SQLite behavior).
        let result = connection.execute(
            "INSERT INTO invCategories (categoryId, categoryName, published) \
             VALUES (1, 'Test', 'yes')",
            [],
        );
        assert!(result.is_err());
    }

    #[test]
    fn create_schema_enforces_foreign_keys() {
        // The DDL itself turns on `PRAGMA foreign_keys = ON` as its
        // first statement, so there's no need to enable it separately
        // before calling create_schema().
        let connection = Connection::open_in_memory().unwrap();
        create_schema(&connection).unwrap();

        // invGroups.categoryId references invCategories(categoryId); no
        // category with id 999 exists, so this INSERT must be rejected.
        let result = connection.execute(
            "INSERT INTO invGroups (groupId, groupName, categoryId, anchorable) \
             VALUES (1, 'Test', 999, 0)",
            [],
        );
        assert!(result.is_err());
    }

    #[test]
    fn create_schema_accepts_valid_rows_respecting_fk_order() {
        // End-to-end smoke test: a valid row on each end of an FK
        // relationship (invCategories -> invGroups) must insert without
        // issues when the parent is created first.
        let connection = Connection::open_in_memory().unwrap();
        create_schema(&connection).unwrap();

        connection
            .execute(
                "INSERT INTO invCategories (categoryId, categoryName, published) \
                 VALUES (1, 'Celestial', 1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO invGroups (groupId, groupName, categoryId, anchorable) \
                 VALUES (10, 'Sun', 1, 0)",
                [],
            )
            .unwrap();

        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM invGroups", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }
}
