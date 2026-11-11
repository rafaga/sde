//! `mapPlanets`.

use super::*;

impl Parser {
    /// Populates `mapPlanets` from `<sde_directory>/mapPlanets.jsonl`,
    /// filtering by `state.systems_in_scope` (populated by
    /// [`Self::parse_solar_systems`]). Requires `mapSolarSystems`/`invTypes` to
    /// already be populated (FKs). Returns the number of rows inserted.
    ///
    /// Confirmed against a real sample of `mapPlanets.jsonl` (68407
    /// records, EVE Online, August 2026):
    /// - `celestialIndex`, `position`, `typeID` and `solarSystemID` are
    ///   present in 100% of records, so they're treated as required
    ///   (`required_i64`/`required_position`), same criterion used
    ///   throughout this file for `NOT NULL` columns
    ///   (`mapPlanets.planetaryIndex` is one) when the real source
    ///   confirms the data is always there: fail early with a clear
    ///   message instead of letting SQLite reject a `NULL` further down.
    /// - `radius` is **always** at the top level (never needs the nested
    ///   fallback to `statistics.radius`) -- but unlike `mapStars.radius`
    ///   (an `INTEGER` column), `mapPlanets.radius` is `REAL`, so it's
    ///   read with `optional_f64_with_nested_fallback`, not the `i64`
    ///   variant.
    /// - `fragmented` **never** shows up, neither at the top level nor
    ///   nested (0 out of 68407) -- in practice this column always comes
    ///   out `NULL`.
    /// - `locked`, on the other hand, is **always** nested under
    ///   `statistics` (never at the top level) -- the opposite of
    ///   `radius`. Here the fallback genuinely matters, to not lose the
    ///   data.
    #[tracing::instrument(skip(state))]
    pub fn parse_planets(
        &self,
        connection: &Connection,
        state: &SystemScopeState,
    ) -> Result<usize, Error> {
        let mut insert_planet = connection.prepare(
            "INSERT INTO mapPlanets (planetId, solarSystemId, planetaryIndex, fragmented, radius, \
            locked, typeId, positionX, positionY, positionZ) \
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        )?;

        self.for_each_record("mapPlanets", "planets", |record| {
            let solar_system_id = self.required_i64(record, "solarSystemID")?;
            if !state.systems_in_scope.contains(&solar_system_id) {
                return Ok(false);
            }

            let id = self.required_i64(record, "_key")?;
            let planet_index = self.required_i64(record, "celestialIndex")?;
            let fragmented =
                self.optional_bool_with_nested_fallback(record, "fragmented", "statistics");
            let radius = self.optional_f64_with_nested_fallback(record, "radius", "statistics");
            let locked = self.optional_bool_with_nested_fallback(record, "locked", "statistics");
            let type_id = self.required_i64(record, "typeID")?;
            let (pos_x, pos_y, pos_z) = self.required_position(record)?;

            insert_planet.execute(rusqlite::params![
                id,
                solar_system_id,
                planet_index,
                fragmented,
                radius,
                locked,
                type_id,
                pos_x,
                pos_y,
                pos_z,
            ])?;
            Ok(true)
        })
    }

    // mapMoons
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::parser::test_support::TempSdeDir;

    /// Common setup for `parse_planets`'s tests: schema, a minimal
    /// `invTypes` to satisfy `typeId`'s FK, and a solar system in
    /// scope. Returns `(connection, scope, config)`.
    fn setup_for_parse_planets() -> (Connection, SystemScopeState, ParserConfig) {
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
                 VALUES (1, 1, 'Planet', 0)",
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

        let mut scope = SystemScopeState::default();
        scope.systems_in_scope.insert(30000001);
        let config = ParserConfig::default();
        (connection, scope, config)
    }

    #[test]
    fn parse_planets_inserts_row_using_real_sde_shape() {
        // Real mapPlanets.jsonl record (August 2026, EVE Online):
        // celestialIndex/position/typeID/solarSystemID always present;
        // radius at the top level; locked ALWAYS nested under
        // statistics (never at the top level); fragmented absent.
        let dir = TempSdeDir::new(
            "planets_real_shape",
            &[(
                "mapPlanets.jsonl",
                "{\"_key\": 40000002, \"celestialIndex\": 1, \
                 \"position\": {\"x\": 161891117336.0, \"y\": 21288951986.0, \"z\": -73529712226.0}, \
                 \"radius\": 5060000, \"solarSystemID\": 30000001, \
                 \"statistics\": {\"locked\": false}, \"typeID\": 11}\n",
            )],
        );
        let (connection, scope, config) = setup_for_parse_planets();
        let parser = Parser::new(&dir.path, config);

        let count = parser.parse_planets(&connection, &scope).unwrap();
        assert_eq!(count, 1);

        let (planetary_index, fragmented, radius, locked, type_id): (
            i64,
            Option<i64>,
            f64,
            i64,
            i64,
        ) = connection
            .query_row(
                "SELECT planetaryIndex, fragmented, radius, locked, typeId \
                 FROM mapPlanets WHERE planetId = 40000002",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(planetary_index, 1);
        assert_eq!(fragmented, None);
        assert_eq!(radius, 5060000.0);
        assert_eq!(locked, 0);
        assert_eq!(type_id, 11);
    }

    #[test]
    fn parse_planets_skips_systems_outside_scope() {
        let dir = TempSdeDir::new(
            "planets_scope",
            &[(
                "mapPlanets.jsonl",
                "{\"_key\": 40000002, \"celestialIndex\": 1, \
                 \"position\": {\"x\": 0.0, \"y\": 0.0, \"z\": 0.0}, \
                 \"radius\": 1, \"solarSystemID\": 30000099, \"typeID\": 11}\n",
            )],
        );
        let (connection, scope, config) = setup_for_parse_planets();
        let parser = Parser::new(&dir.path, config);
        // 30000099 is not in scope (only 30000001 is).

        let count = parser.parse_planets(&connection, &scope).unwrap();
        assert_eq!(count, 0);

        let total: i64 = connection
            .query_row("SELECT COUNT(*) FROM mapPlanets", [], |row| row.get(0))
            .unwrap();
        assert_eq!(total, 0);
    }

    #[test]
    fn parse_planets_missing_celestial_index_errors() {
        let dir = TempSdeDir::new(
            "planets_missing_index",
            &[(
                "mapPlanets.jsonl",
                "{\"_key\": 40000002, \"position\": {\"x\": 0.0, \"y\": 0.0, \"z\": 0.0}, \
                 \"radius\": 1, \"solarSystemID\": 30000001, \"typeID\": 11}\n",
            )],
        );
        let (connection, scope, config) = setup_for_parse_planets();
        let parser = Parser::new(&dir.path, config);

        let result = parser.parse_planets(&connection, &scope);
        assert!(result.is_err());
    }

    #[test]
    fn parse_planets_missing_position_errors() {
        let dir = TempSdeDir::new(
            "planets_missing_position",
            &[(
                "mapPlanets.jsonl",
                "{\"_key\": 40000002, \"celestialIndex\": 1, \
                 \"radius\": 1, \"solarSystemID\": 30000001, \"typeID\": 11}\n",
            )],
        );
        let (connection, scope, config) = setup_for_parse_planets();
        let parser = Parser::new(&dir.path, config);

        let result = parser.parse_planets(&connection, &scope);
        assert!(result.is_err());
    }
}
