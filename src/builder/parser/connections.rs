//! `mapSystemConnections`, derived from the gates.

use super::*;

impl Parser {
    /// Populates `mapSystemConnections` from `mapSystemGates`, joining each
    /// gate with the gate it points to (`destinationGateId`) to derive the
    /// pair of solar systems it connects. Unlike every other function in
    /// this file, this one does NOT read any SDE file -- the whole logic
    /// is a single SQL statement over data already inserted by
    /// [`Self::parse_stargates`], which is why it doesn't take `sde_directory`.
    /// Returns the number of rows inserted.
    ///
    /// Requires `mapSystemGates` to already be populated. If
    /// `config.with_gates` was `false` (so [`Self::parse_stargates`] never ran)
    /// or there simply were no gates to import, this query finds no rows to
    /// join and inserts nothing -- not an error, it returns `0`.
    ///
    /// The `WHERE msga.solarSystemId < msgb.solarSystemId` filters down to
    /// a single record per connected system pair: stargates always come in
    /// mutual pairs (A points to B, B points to A), so without this filter
    /// each connection would get inserted twice (once per direction),
    /// violating the schema's `CHECK (systemA < systemB)` on the second
    /// attempt. The statement's `MIN`/`MAX` are the 2-argument scalar form
    /// (not the 1-argument aggregate form used elsewhere in this crate,
    /// e.g. in `get_region_coordinates` in `src/lib.rs`) -- they compute
    /// the min/max *per row*, not across rows; given the `WHERE` above,
    /// they always end up returning
    /// `(msga.solarSystemId, msgb.solarSystemId)` in that order.
    #[tracing::instrument]
    pub fn parse_connections(&self, connection: &Connection) -> Result<usize, Error> {
        let count = connection.execute(
            "INSERT INTO mapSystemConnections (systemA, systemB) \
            SELECT MIN(msga.solarSystemId, msgb.solarSystemId), \
                    MAX(msga.solarSystemId, msgb.solarSystemId) \
            FROM mapSystemGates AS msga \
            INNER JOIN mapSystemGates AS msgb ON (msgb.systemGateId = msga.destinationGateId) \
            WHERE msga.solarSystemId < msgb.solarSystemId",
            [],
        )?;
        if self.config.verbose {
            tracing::info!("Parsed {count} system connections");
        }
        Ok(count)
    }

    // stationServices / stationOperations / npcStations
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_connections_derives_single_pair_from_mutual_gates() {
        let mut connection = Connection::open_in_memory().unwrap();
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
        // On purpose, the gate with the SMALLER solarSystemId
        // (30000001) is inserted as row #2, and the one with the
        // LARGER solarSystemId (30000002) as row #1, to confirm that
        // insertion order doesn't affect the result.
        for (id, name) in [(30000002, "B"), (30000001, "A")] {
            connection
                .execute(
                    "INSERT INTO mapSolarSystems \
                     (solarSystemId, solarSystemName, constellationId, radius, centerX, centerY, centerZ, security) \
                     VALUES (?1, ?2, 20000020, 1.0, 0, 0, 0, 0.5)",
                    rusqlite::params![id, name],
                )
                .unwrap();
        }
        // The two gates reference each other mutually
        // (destinationGateId), and that FK is DEFERRABLE INITIALLY
        // DEFERRED -- exactly the case documented in parse_stargates's
        // own docstring: outside an explicit transaction, each INSERT
        // is its own implicit transaction in autocommit mode, so the
        // first gate inserted fails immediately (its destinationGateId
        // doesn't exist yet). Wrapping both inserts in one transaction
        // defers the FK check until the commit, by which point both
        // gates exist.
        {
            let tx = connection.transaction().unwrap();
            tx.execute(
                "INSERT INTO mapSystemGates \
                 (systemGateId, solarSystemId, typeId, positionX, positionY, positionZ, destinationGateId, destinationSystemId) \
                 VALUES (50000001, 30000002, 16, 0, 0, 0, 50000002, 30000001)",
                [],
            )
            .unwrap();
            tx.execute(
                "INSERT INTO mapSystemGates \
                 (systemGateId, solarSystemId, typeId, positionX, positionY, positionZ, destinationGateId, destinationSystemId) \
                 VALUES (50000002, 30000001, 16, 0, 0, 0, 50000001, 30000002)",
                [],
            )
            .unwrap();
            tx.commit().unwrap();
        }

        let config = ParserConfig::default();
        // No TempSdeDir here -- parse_connections() derives everything
        // from mapSystemGates (already in `connection`), never reads
        // self.sde_directory, so the path is never actually used.
        let parser = Parser::new(Path::new("."), config);
        let count = parser.parse_connections(&connection).unwrap();
        assert_eq!(count, 1);

        let (system_a, system_b): (i64, i64) = connection
            .query_row(
                "SELECT systemA, systemB FROM mapSystemConnections",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        // systemA < systemB, regardless of the gates' insertion order.
        assert_eq!((system_a, system_b), (30000001, 30000002));
    }

    #[test]
    fn parse_connections_returns_zero_when_no_gates() {
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();

        let config = ParserConfig::default();
        // Same as above: no file ever read, path is just a placeholder.
        let parser = Parser::new(Path::new("."), config);
        let count = parser.parse_connections(&connection).unwrap();
        assert_eq!(count, 0);

        let total: i64 = connection
            .query_row("SELECT COUNT(*) FROM mapSystemConnections", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(total, 0);
    }
}
