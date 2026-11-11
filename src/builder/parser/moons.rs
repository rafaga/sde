//! `mapMoons`.

use super::*;

impl Parser {
    /// Populates `mapMoons` from `<sde_directory>/mapMoons.jsonl`, filtering
    /// by `state.systems_in_scope` (populated by [`Self::parse_solar_systems`]).
    /// Requires `mapSolarSystems` to already be populated (FK). Returns the
    /// number of rows inserted.
    ///
    /// Confirmed against a real sample of `mapMoons.jsonl` (344457
    /// records, EVE Online, August 2026): `celestialIndex`, `orbitID`,
    /// `orbitIndex`, `typeID`, `position` and `solarSystemID` are present
    /// in 100% of records. `locked` is never at the top level, nested
    /// under `statistics` in 99.6% of records -- but genuinely absent
    /// from both places in the remaining 0.4% (1364 of 344457), confirming
    /// the nested fallback (see `optional_bool_with_nested_fallback`) is
    /// exercised by real data, not just a theoretical possibility.
    ///
    /// `moonIndex` (`orbitIndex` in the JSON) is treated as required
    /// (`required_i64`) -- confirmed present in every one of the 344457
    /// real records checked, same criterion as `mapPlanets.planetaryIndex`.
    ///
    /// `typeId` is also treated as required (`required_i64`), confirmed
    /// present in every real record checked -- even though the column
    /// itself is nullable in the schema (`typeId INTEGER REFERENCES
    /// invTypes(typeId)`, without `NOT NULL`).
    ///
    /// Real moon `position` magnitude checked too: up to ~1.8x10^13 in the
    /// sample (about 0.2% of 2^53) -- far below the `i64 -> f64` precision
    /// boundary discussed in [`crate::objects::SdePoint`]'s docstring.
    /// Moon positions are system-scale (similar to `mapPlanets`'s
    /// ~3x10^13), not galactic-scale like `mapRegions`/`mapSolarSystems`'s
    /// ~10^19 -- no precision concern here, for this data or for any
    /// future function that might expose it (`SdeManager::get_moon()`
    /// doesn't read `position` today; `objects::Moon` has no coordinate
    /// field to put it in).
    #[tracing::instrument(skip(state))]
    pub fn parse_moons(
        &self,
        connection: &Connection,
        state: &SystemScopeState,
    ) -> Result<usize, Error> {
        let mut insert_moon = connection.prepare(
            "INSERT INTO mapMoons (moonId, solarSystemId, moonIndex, planetId, typeId, radius, \
            positionX, positionY, positionZ) \
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )?;

        self.for_each_record("mapMoons", "moons", |record| {
            let solar_system_id = self.required_i64(record, "solarSystemID")?;
            if !state.systems_in_scope.contains(&solar_system_id) {
                return Ok(false);
            }

            let id = self.required_i64(record, "_key")?;
            let moon_index = self.required_i64(record, "orbitIndex")?;
            let planet_id = self.optional_i64(record, "orbitID");
            let type_id = self.required_i64(record, "typeID")?;
            let radius = self.optional_i64_with_nested_fallback(record, "radius", "statistics");
            let (pos_x, pos_y, pos_z) = self.required_position(record)?;

            insert_moon.execute(rusqlite::params![
                id,
                solar_system_id,
                moon_index,
                planet_id,
                type_id,
                radius,
                pos_x,
                pos_y,
                pos_z,
            ])?;
            Ok(true)
        })
    }

    // mapSystemConnections
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::parser::test_support::TempSdeDir;

    /// Common setup for `parse_moons`'s tests: schema, an `invTypes`
    /// row for the planet and another for the moon, a solar system and
    /// a planet in scope (so `planetId` can be tested with a real value
    /// as well as `NULL`). Returns `(connection, scope, config)`.
    fn setup_for_parse_moons() -> (Connection, SystemScopeState, ParserConfig) {
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        connection
            .execute(
                "INSERT INTO invCategories (categoryId, categoryName, published) \
                 VALUES (1, 'Celestial', 1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO invGroups (groupId, categoryId, groupName, anchorable) \
                 VALUES (1, 1, 'Celestial Group', 0)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO invTypes (typeId, groupId, typeName, published) \
                 VALUES (11, 1, 'Planet (Barren)', 1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO invTypes (typeId, groupId, typeName, published) \
                 VALUES (12, 1, 'Moon', 1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO mapRegions \
                 (regionId, regionName, factionId, centerX, centerY, centerZ, nebula, wormholeClassId) \
                 VALUES (10000002, 'The Forge', NULL, 0, 0, 0, 5, NULL)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO mapConstellations \
                 (constellationId, constellationName, regionId, centerX, centerY, centerZ) \
                 VALUES (20000020, 'Kimotoro', 10000002, 0, 0, 0)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO mapSolarSystems \
                 (solarSystemId, solarSystemName, constellationId, radius, centerX, centerY, centerZ, security) \
                 VALUES (30000001, 'A', 20000020, 1.0, 0, 0, 0, 0.5)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO mapPlanets \
                 (planetId, solarSystemId, planetaryIndex, typeId, positionX, positionY, positionZ) \
                 VALUES (40000002, 30000001, 1, 11, 0, 0, 0)",
                [],
            )
            .unwrap();

        let mut scope = SystemScopeState::default();
        scope.systems_in_scope.insert(30000001);
        let config = ParserConfig::default();
        (connection, scope, config)
    }

    #[test]
    fn parse_moons_inserts_row_with_planet_reference() {
        let dir = TempSdeDir::new(
            "moons_with_planet",
            &[(
                "mapMoons.jsonl",
                "{\"_key\": 40000004, \"solarSystemID\": 30000001, \"orbitIndex\": 1, \
                 \"orbitID\": 40000002, \"typeID\": 12, \"radius\": 100000, \
                 \"position\": {\"x\": 1.0, \"y\": 2.0, \"z\": 3.0}}\n",
            )],
        );
        let (connection, scope, config) = setup_for_parse_moons();
        let parser = Parser::new(&dir.path, config);

        let count = parser.parse_moons(&connection, &scope).unwrap();
        assert_eq!(count, 1);

        let (moon_index, planet_id, type_id, radius): (i64, Option<i64>, i64, Option<i64>) =
            connection
                .query_row(
                    "SELECT moonIndex, planetId, typeId, radius FROM mapMoons WHERE moonId = 40000004",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .unwrap();
        assert_eq!(moon_index, 1);
        assert_eq!(planet_id, Some(40000002));
        assert_eq!(type_id, 12);
        assert_eq!(radius, Some(100000));
    }

    #[test]
    fn parse_moons_without_orbit_id_leaves_planet_id_null() {
        // orbitID (planetId) is optional in the schema (a nullable
        // column).
        let dir = TempSdeDir::new(
            "moons_no_planet",
            &[(
                "mapMoons.jsonl",
                "{\"_key\": 40000005, \"solarSystemID\": 30000001, \"orbitIndex\": 2, \
                 \"typeID\": 12, \"position\": {\"x\": 0.0, \"y\": 0.0, \"z\": 0.0}}\n",
            )],
        );
        let (connection, scope, config) = setup_for_parse_moons();
        let parser = Parser::new(&dir.path, config);

        parser.parse_moons(&connection, &scope).unwrap();

        let planet_id: Option<i64> = connection
            .query_row(
                "SELECT planetId FROM mapMoons WHERE moonId = 40000005",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(planet_id, None);
    }

    #[test]
    fn parse_moons_skips_systems_outside_scope() {
        let dir = TempSdeDir::new(
            "moons_scope",
            &[(
                "mapMoons.jsonl",
                "{\"_key\": 40000004, \"solarSystemID\": 30000099, \"orbitIndex\": 1, \
                 \"typeID\": 12, \"position\": {\"x\": 0.0, \"y\": 0.0, \"z\": 0.0}}\n",
            )],
        );
        let (connection, scope, config) = setup_for_parse_moons();
        let parser = Parser::new(&dir.path, config);
        // 30000099 is not in scope (only 30000001 is).

        let count = parser.parse_moons(&connection, &scope).unwrap();
        assert_eq!(count, 0);

        let total: i64 = connection
            .query_row("SELECT COUNT(*) FROM mapMoons", [], |row| row.get(0))
            .unwrap();
        assert_eq!(total, 0);
    }

    #[test]
    fn parse_moons_missing_orbit_index_errors() {
        let dir = TempSdeDir::new(
            "moons_missing_index",
            &[(
                "mapMoons.jsonl",
                "{\"_key\": 40000004, \"solarSystemID\": 30000001, \"typeID\": 12, \
                 \"position\": {\"x\": 0.0, \"y\": 0.0, \"z\": 0.0}}\n",
            )],
        );
        let (connection, scope, config) = setup_for_parse_moons();
        let parser = Parser::new(&dir.path, config);

        let result = parser.parse_moons(&connection, &scope);
        assert!(result.is_err());
    }

    #[test]
    fn parse_moons_missing_type_id_errors() {
        let dir = TempSdeDir::new(
            "moons_missing_type",
            &[(
                "mapMoons.jsonl",
                "{\"_key\": 40000004, \"solarSystemID\": 30000001, \"orbitIndex\": 1, \
                 \"position\": {\"x\": 0.0, \"y\": 0.0, \"z\": 0.0}}\n",
            )],
        );
        let (connection, scope, config) = setup_for_parse_moons();
        let parser = Parser::new(&dir.path, config);

        let result = parser.parse_moons(&connection, &scope);
        assert!(result.is_err());
    }
}
