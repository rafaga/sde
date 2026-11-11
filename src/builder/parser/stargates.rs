//! `mapSystemGates`.

use super::*;

impl Parser {
    /// Populates `mapSystemGates` from `<sde_directory>/mapStargates.jsonl`
    /// (the file is named `mapStargates`, even though the destination table
    /// is `mapSystemGates` -- that's how the SDE itself names it). Filters
    /// by `state.systems_in_scope` (populated by [`Self::parse_solar_systems`]): a
    /// gate whose `solarSystemID` isn't in that set is skipped. Requires
    /// `mapSolarSystems`/`invTypes` to already be populated (FKs). Returns
    /// the number of rows inserted.
    ///
    /// # Important: requires an explicit transaction
    ///
    /// `mapSystemGates.destinationGateId` references another row of the
    /// SAME table (`systemGateId`), declared `DEFERRABLE INITIALLY
    /// DEFERRED` in the schema -- that lets SQLite postpone that FK's
    /// validation until the transaction's `COMMIT`, instead of requiring
    /// the destination gate to already exist at the exact moment of the
    /// INSERT. This matters because stargates usually come in pairs that
    /// reference each other mutually (A's gate points to B's, and vice
    /// versa), so whatever order the file is in, the first of the two to be
    /// inserted necessarily references one that doesn't exist yet.
    ///
    /// Verified empirically (sqlite3 with `isolation_level=None`, which
    /// replicates SQLite/rusqlite's real autocommit mode): inserting that
    /// first gate **outside** an explicit transaction fails with
    /// `FOREIGN KEY constraint failed` -- in autocommit mode each `INSERT`
    /// is its own implicit transaction, so the deferred validation still
    /// fires immediately, when that single statement's transaction closes.
    /// Wrapped in an explicit transaction (`BEGIN`/`COMMIT`), on the other
    /// hand, both INSERTs resolve correctly because validation is postponed
    /// until the final `COMMIT`, by which point both gates already exist.
    ///
    /// In practice this means calling this function on its own (outside of
    /// [`Self::parse_data`], without going through `Connection::transaction()`)
    /// doesn't just lose the "all or nothing" atomicity guarantee already
    /// documented for the rest of the pipeline (see "Notable behavior" in the
    /// module's docstring) -- here it can make the insertion of perfectly
    /// valid data fail, purely because of the order records appear in the
    /// file.
    #[tracing::instrument(skip(state))]
    pub fn parse_stargates(
        &self,
        connection: &Connection,
        state: &SystemScopeState,
    ) -> Result<usize, Error> {
        let mut insert_gate = connection.prepare(
            "INSERT INTO mapSystemGates (systemGateId, solarSystemId, typeId, \
            positionX, positionY, positionZ, destinationGateId, destinationSystemId) \
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;

        self.for_each_record("mapStargates", "stargates", |record| {
            let solar_system_id = self.required_i64(record, "solarSystemID")?;
            if !state.systems_in_scope.contains(&solar_system_id) {
                return Ok(false);
            }

            let id = self.required_i64(record, "_key")?;
            let type_id = self.required_i64(record, "typeID")?;
            let (pos_x, pos_y, pos_z) = self.required_position(record)?;
            let destination_gate_id =
                self.required_nested_i64(record, "destination", "stargateID")?;
            let destination_system_id =
                self.required_nested_i64(record, "destination", "solarSystemID")?;

            insert_gate.execute(rusqlite::params![
                id,
                solar_system_id,
                type_id,
                pos_x,
                pos_y,
                pos_z,
                destination_gate_id,
                destination_system_id,
            ])?;
            Ok(true)
        })
    }

    // mapStars
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::parser::test_support::TempSdeDir;

    /// FK prerequisites shared by `parse_stargates`'s tests: two solar
    /// systems (30000001, 30000002) in the same constellation, and the
    /// item type (16, "Stargate") referenced by `mapSystemGates.typeId`.
    fn insert_stargate_prerequisites(connection: &Connection) {
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
                 VALUES (1, 1, 'Stargate Group', 0)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO invTypes (typeId, groupId, typeName, published) \
                 VALUES (16, 1, 'Stargate', 1)",
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
        for (id, name) in [(30000001, "A"), (30000002, "B")] {
            connection
                .execute(
                    "INSERT INTO mapSolarSystems \
                     (solarSystemId, solarSystemName, constellationId, radius, centerX, centerY, centerZ, security) \
                     VALUES (?1, ?2, 20000020, 1.0, 0, 0, 0, 0.5)",
                    rusqlite::params![id, name],
                )
                .unwrap();
        }
    }

    /// Fixture of two mutually-referencing stargates: gate 50000001 (in
    /// system 30000001) points to 50000002 (in 30000002), and vice
    /// versa -- the typical case in real SDE data.
    const MUTUAL_STARGATES_JSONL: &str = "{\"_key\": 50000001, \"solarSystemID\": 30000001, \"typeID\": 16, \
         \"position\": {\"x\": 1.0, \"y\": 2.0, \"z\": 3.0}, \
         \"destination\": {\"stargateID\": 50000002, \"solarSystemID\": 30000002}}\n\
         {\"_key\": 50000002, \"solarSystemID\": 30000002, \"typeID\": 16, \
         \"position\": {\"x\": 4.0, \"y\": 5.0, \"z\": 6.0}, \
         \"destination\": {\"stargateID\": 50000001, \"solarSystemID\": 30000001}}\n";

    #[test]
    fn parse_stargates_without_transaction_fails_on_mutual_reference() {
        // Documents the behavior described in parse_stargates's
        // docstring: without an explicit transaction, SQLite operates
        // in autocommit mode (each INSERT is its own implicit
        // transaction), so destinationGateId's DEFERRABLE FK still
        // gets validated immediately -- and the first gate of the pair
        // necessarily references one that doesn't exist yet.
        let dir = TempSdeDir::new(
            "stargates_no_tx",
            &[("mapStargates.jsonl", MUTUAL_STARGATES_JSONL)],
        );
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        insert_stargate_prerequisites(&connection);
        let mut scope = SystemScopeState::default();
        scope.systems_in_scope.insert(30000001);
        scope.systems_in_scope.insert(30000002);

        let config = ParserConfig::default();
        let parser = Parser::new(&dir.path, config);
        let result = parser.parse_stargates(&connection, &scope);
        assert!(result.is_err());
    }

    #[test]
    fn parse_stargates_within_transaction_inserts_mutual_reference() {
        let dir = TempSdeDir::new(
            "stargates_tx",
            &[("mapStargates.jsonl", MUTUAL_STARGATES_JSONL)],
        );
        let mut connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        insert_stargate_prerequisites(&connection);
        let mut scope = SystemScopeState::default();
        scope.systems_in_scope.insert(30000001);
        scope.systems_in_scope.insert(30000002);

        let tx = connection.transaction().unwrap();
        let config = ParserConfig::default();
        let parser = Parser::new(&dir.path, config);
        let count = parser.parse_stargates(&tx, &scope).unwrap();
        assert_eq!(count, 2);
        tx.commit().unwrap();

        let (dest_gate, dest_system): (i64, i64) = connection
            .query_row(
                "SELECT destinationGateId, destinationSystemId FROM mapSystemGates \
                 WHERE systemGateId = 50000001",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(dest_gate, 50000002);
        assert_eq!(dest_system, 30000002);
    }

    #[test]
    fn parse_stargates_skips_systems_outside_scope() {
        let dir = TempSdeDir::new(
            "stargates_scope",
            &[(
                "mapStargates.jsonl",
                "{\"_key\": 50000003, \"solarSystemID\": 30000003, \"typeID\": 16, \
                 \"position\": {\"x\": 0.0, \"y\": 0.0, \"z\": 0.0}, \
                 \"destination\": {\"stargateID\": 50000004, \"solarSystemID\": 30000001}}\n",
            )],
        );
        let mut connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        insert_stargate_prerequisites(&connection);
        // 30000003 is NOT in scope (unlike 30000001/30000002).
        let mut scope = SystemScopeState::default();
        scope.systems_in_scope.insert(30000001);
        scope.systems_in_scope.insert(30000002);

        let tx = connection.transaction().unwrap();
        let config = ParserConfig::default();
        let parser = Parser::new(&dir.path, config);
        let count = parser.parse_stargates(&tx, &scope).unwrap();
        tx.commit().unwrap();
        assert_eq!(count, 0);

        let total: i64 = connection
            .query_row("SELECT COUNT(*) FROM mapSystemGates", [], |row| row.get(0))
            .unwrap();
        assert_eq!(total, 0);
    }

    #[test]
    fn parse_stargates_missing_type_id_errors() {
        let dir = TempSdeDir::new(
            "stargates_missing_type",
            &[(
                "mapStargates.jsonl",
                "{\"_key\": 50000001, \"solarSystemID\": 30000001, \
                 \"position\": {\"x\": 0.0, \"y\": 0.0, \"z\": 0.0}, \
                 \"destination\": {\"stargateID\": 50000002, \"solarSystemID\": 30000002}}\n",
            )],
        );
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        insert_stargate_prerequisites(&connection);
        let mut scope = SystemScopeState::default();
        scope.systems_in_scope.insert(30000001);

        let config = ParserConfig::default();
        let parser = Parser::new(&dir.path, config);
        let result = parser.parse_stargates(&connection, &scope);
        assert!(result.is_err());
    }
}
