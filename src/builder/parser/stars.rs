//! `mapStars`.

use super::*;

impl Parser {
    /// Populates `mapStars` from `<sde_directory>/mapStars.jsonl`, filtering
    /// by `state.systems_in_scope` (populated by [`Self::parse_solar_systems`]).
    /// Requires [`Self::parse_types`] to have already run -- it needs
    /// `star_state.star_type_ids`, the set of `typeId`s [`Self::parse_types`]
    /// inserted into `typeStar` -- and `mapSolarSystems`/`typeStar` to
    /// already be populated (FKs).
    /// Returns the number of rows inserted.
    ///
    /// Confirmed against a real sample of `mapStars.jsonl` (8089
    /// records, EVE Online, August 2026): `radius` always comes at the
    /// top level as an integer (never needs the nested fallback to
    /// `statistics.radius`), `statistics` is always present, and `locked`
    /// **never** shows up -- neither at the top level nor inside
    /// `statistics` -- so in practice that column always comes out
    /// `NULL`. The nested fallback (see `optional_i64_with_nested_fallback`/
    /// `optional_bool_with_nested_fallback`) is kept anyway, in case some
    /// other SDE version does carry it.
    ///
    /// # Unrecognized `typeID`
    ///
    /// If a star's `typeID` isn't in `star_state.star_type_ids` (meaning
    /// [`Self::parse_types`] didn't detect it as belonging to the "Sun"
    /// group), that's a direct `Error::data` -- same criterion as
    /// the rest of this file: fail early with a clear message instead of
    /// letting SQLite reject a value that was going to be invalid anyway
    /// (a raw, unrecognized `typeID` would violate the
    /// `mapStars.starTypeId -> typeStar.typeId` FK).
    #[tracing::instrument(skip(state, star_state))]
    pub fn parse_stars(
        &self,
        connection: &Connection,
        state: &SystemScopeState,
        star_state: &StarTypeState,
    ) -> Result<usize, Error> {
        let mut insert_star = connection.prepare(
            "INSERT INTO mapStars (starId, solarSystemId, locked, radius, starTypeId) \
            VALUES (?1, ?2, ?3, ?4, ?5)",
        )?;

        self.for_each_record("mapStars", "stars", |record| {
            let solar_system_id = self.required_i64(record, "solarSystemID")?;
            if !state.systems_in_scope.contains(&solar_system_id) {
                return Ok(false);
            }

            let star_id = self.required_i64(record, "_key")?;
            let locked = self.optional_bool_with_nested_fallback(record, "locked", "statistics");
            let radius = self.optional_i64_with_nested_fallback(record, "radius", "statistics");
            let type_id = self.required_i64(record, "typeID")?;
            // typeStar's primary key *is* typeId (no separate,
            // self-assigned id to translate into), so the only thing
            // left to check is that this typeId was actually recognized
            // as a star type -- the value written to
            // mapStars.starTypeId is type_id itself, unchanged.
            if !star_state.star_type_ids.contains(&type_id) {
                return Err(Error::data(format!(
                    "star {star_id}: typeId {type_id} isn't in star_type_ids \
                    (parse_types() didn't detect it as a star type)"
                )));
            }
            let star_type_id = type_id;

            insert_star.execute(rusqlite::params![
                star_id,
                solar_system_id,
                locked,
                radius,
                star_type_id
            ])?;
            Ok(true)
        })
    }

    // mapPlanets
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::parser::test_support::TempSdeDir;

    /// Common setup for `parse_stars`'s tests: creates the schema, a
    /// detected star type ("Sun" > "Yellow G5 (ffcc00)") via
    /// `parse_groups`/`parse_types` directly against dedicated fixtures
    /// (to get a real `StarTypeState`, not a hand-simulated one), and a
    /// solar system in scope. Returns `(connection, star_state, scope)`.
    fn setup_for_parse_stars(
        dir_prefix: &str,
    ) -> (Connection, StarTypeState, SystemScopeState, ParserConfig) {
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();

        let types_dir = TempSdeDir::new(
            dir_prefix,
            &[
                (
                    "groups.jsonl",
                    "{\"_key\": 6, \"categoryID\": 6, \"name\": {\"en\": \"Sun\"}, \"anchorable\": false}\n",
                ),
                (
                    "types.jsonl",
                    "{\"_key\": 3000, \"groupID\": 6, \"name\": {\"en\": \"Yellow G5 (ffcc00)\"}, \
                     \"iconID\": 100, \"published\": true, \"volume\": 0.0}\n",
                ),
            ],
        );
        connection
            .execute(
                "INSERT INTO invCategories (categoryId, categoryName, published) \
                 VALUES (6, 'Celestial', 1)",
                [],
            )
            .unwrap();
        let config = ParserConfig::default();
        let mut star_state = StarTypeState::default();
        let types_parser = Parser::new(&types_dir.path, config.clone());
        types_parser
            .parse_groups(&connection, &mut star_state)
            .unwrap();
        types_parser
            .parse_types(&connection, &mut star_state)
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

        (connection, star_state, scope, config)
    }

    #[test]
    fn parse_stars_inserts_row_using_real_sde_shape() {
        // Record with the shape confirmed against a real sample of
        // mapStars.jsonl (August 2026): radius as a top-level integer,
        // statistics present, locked nowhere to be found.
        let dir = TempSdeDir::new(
            "stars_real_shape",
            &[(
                "mapStars.jsonl",
                "{\"_key\": 40000001, \"radius\": 63350000, \"solarSystemID\": 30000001, \
                 \"statistics\": {\"age\": 4.5e17, \"life\": 6.9e17, \"luminosity\": 0.01575, \
                 \"spectralClass\": \"K2 V\", \"temperature\": 4567.0}, \"typeID\": 3000}\n",
            )],
        );
        let (connection, star_state, scope, config) = setup_for_parse_stars("stars_setup_real");
        let parser = Parser::new(&dir.path, config);

        let count = parser
            .parse_stars(&connection, &scope, &star_state)
            .unwrap();
        assert_eq!(count, 1);

        let (solar_system_id, locked, radius): (i64, Option<i64>, i64) = connection
            .query_row(
                "SELECT solarSystemId, locked, radius FROM mapStars WHERE starId = 40000001",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(solar_system_id, 30000001);
        assert_eq!(locked, None);
        assert_eq!(radius, 63350000);
    }

    #[test]
    fn parse_stars_locked_falls_back_to_nested_statistics() {
        // Synthetic -- the real SDE never carries `locked` (neither at
        // the top level nor in `statistics`), but the fallback is
        // kept anyway, in case some other SDE version does carry it.
        let dir = TempSdeDir::new(
            "stars_locked_fallback",
            &[(
                "mapStars.jsonl",
                "{\"_key\": 40000001, \"solarSystemID\": 30000001, \"typeID\": 3000, \
                 \"statistics\": {\"locked\": true}}\n",
            )],
        );
        let (connection, star_state, scope, config) = setup_for_parse_stars("stars_setup_fallback");
        let parser = Parser::new(&dir.path, config);

        parser
            .parse_stars(&connection, &scope, &star_state)
            .unwrap();

        let locked: Option<i64> = connection
            .query_row(
                "SELECT locked FROM mapStars WHERE starId = 40000001",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(locked, Some(1));
    }

    #[test]
    fn parse_stars_skips_systems_outside_scope() {
        let dir = TempSdeDir::new(
            "stars_scope",
            &[(
                "mapStars.jsonl",
                "{\"_key\": 40000001, \"radius\": 1, \"solarSystemID\": 30000099, \"typeID\": 3000}\n",
            )],
        );
        let (connection, star_state, scope, config) = setup_for_parse_stars("stars_setup_scope");
        let parser = Parser::new(&dir.path, config);
        // 30000099 is not in scope (only 30000001 is).

        let count = parser
            .parse_stars(&connection, &scope, &star_state)
            .unwrap();
        assert_eq!(count, 0);

        let total: i64 = connection
            .query_row("SELECT COUNT(*) FROM mapStars", [], |row| row.get(0))
            .unwrap();
        assert_eq!(total, 0);
    }

    #[test]
    fn parse_stars_unknown_star_type_errors() {
        let dir = TempSdeDir::new(
            "stars_unknown_type",
            &[(
                "mapStars.jsonl",
                // typeID 9999 is never detected as a star type
                // by parse_types() in this fixture.
                "{\"_key\": 40000001, \"radius\": 1, \"solarSystemID\": 30000001, \"typeID\": 9999}\n",
            )],
        );
        let (connection, star_state, scope, config) = setup_for_parse_stars("stars_setup_unknown");
        let parser = Parser::new(&dir.path, config);

        let result = parser.parse_stars(&connection, &scope, &star_state);
        assert!(result.is_err());
    }
}
