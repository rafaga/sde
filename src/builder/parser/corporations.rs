//! `npcCorporations`, their divisions and junction tables.

use super::*;

/// The prepared `INSERT` statements of [`Parser::parse_npc_corporations`].
struct CorporationStatements<'c> {
    corp: rusqlite::Statement<'c>,
    allowed_race: rusqlite::Statement<'c>,
    division: rusqlite::Statement<'c>,
    trade: rusqlite::Statement<'c>,
    investor: rusqlite::Statement<'c>,
}

impl<'c> CorporationStatements<'c> {
    fn prepare(connection: &'c Connection) -> Result<Self, Error> {
        Ok(Self {
            corp: connection.prepare(
            "INSERT INTO npcCorporations \
            (corporationId, corporationName, tickerName, deleted, description, extent, \
            hasPlayerPersonnelManager, initialPrice, memberLimit, minSecurity, minimumJoinStanding, \
            sendCharTerminationMessage, shares, size, sizeFactor, taxRate, uniqueName, ceoId, \
            mainActivityId, secondaryActivityId, iconId, raceId, enemyId, friendId, factionId, \
            solarSystemId, stationId) \
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, \
                    ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27)",
        )?,
            allowed_race: connection.prepare(
            "INSERT INTO npcCorporationAllowedRaces (corporationId, raceId) VALUES (?1, ?2)",
        )?,
            division: connection.prepare(
            "INSERT INTO npcCorporationDivisionAssignments \
            (corporationId, divisionId, divisionNumber, leaderId, size) \
            VALUES (?1, ?2, ?3, ?4, ?5)",
        )?,
            trade: connection.prepare(
            "INSERT INTO npcCorporationTrades (corporationId, typeId, affinity) VALUES (?1, ?2, ?3)",
        )?,
            investor: connection.prepare(
            "INSERT INTO npcCorporationInvestors (corporationId, investorId, shares) VALUES (?1, ?2, ?3)",
        )?,
        })
    }
}

impl Parser {
    /// Populates `npcCorporationDivisions` from
    /// `<sde_directory>/npcCorporationDivisions.jsonl` (10 records,
    /// confirmed complete for `_key`/`internalName`/`leaderTypeName`).
    #[tracing::instrument]
    pub fn parse_npc_corporation_divisions(&self, connection: &Connection) -> Result<usize, Error> {
        let mut insert = connection.prepare(
            "INSERT INTO npcCorporationDivisions (divisionId, internalName, leaderTypeName) \
            VALUES (?1, ?2, ?3)",
        )?;

        self.for_each_record(
            "npcCorporationDivisions",
            "npcCorporationDivisions",
            |record| {
                let id = self.required_i64(record, "_key")?;
                let internal_name = self.required_str(record, "internalName")?;
                let leader_type_name = self.config.required_localized(record, "leaderTypeName")?;
                insert.execute(rusqlite::params![id, internal_name, leader_type_name])?;
                Ok(true)
            },
        )
    }

    // npcCorporations

    /// Populates `npcCorporations`, `npcCorporationAllowedRaces`,
    /// `npcCorporationDivisionAssignments`, `npcCorporationTrades`, and
    /// `npcCorporationInvestors` from
    /// `<sde_directory>/npcCorporations.jsonl`. Requires `races` and
    /// [`Self::parse_npc_corporation_divisions`] to already be populated.
    /// `enemyId`/`friendId`/`investors` are self-referencing, and
    /// `factionId`/`solarSystemId`/`stationId` reference tables parsed in
    /// later phases -- all `DEFERRABLE`, resolved at `parse_data()`'s final
    /// `COMMIT` (see the relevant columns' comments in `schema.sql`).
    /// Returns the number of `npcCorporations` rows inserted (doesn't count
    /// the four junction tables' rows).
    ///
    /// Rewritten against a real 283-record sample (August 2026) -- the
    /// previous version (`corporationId`/`corporationName`/`tickerName`/
    /// `deleted`/`iconId`/`raceId` only) captured 6 of the real 30 fields.
    /// `lpOfferTables` and `exchangeRates` are the two real fields still not
    /// captured: the former references a "loyalty point offer table"
    /// dataset this project doesn't otherwise have; the latter is present
    /// in only 1 of 283 real records (0.4%), too rare to justify modeling
    /// without a second real example to confirm the shape against.
    /// `ceoID`/`divisions[].leaderID` are kept as plain unconstrained
    /// integers (no character table exists to reference).
    #[tracing::instrument]
    pub fn parse_npc_corporations(&self, connection: &Connection) -> Result<usize, Error> {
        let mut statements = CorporationStatements::prepare(connection)?;
        self.for_each_record("npcCorporations", "npcCorporations", |record| {
            self.insert_corporation(&mut statements, record)
        })
    }

    /// Inserts one `npcCorporations` record and its child rows.
    fn insert_corporation(
        &self,
        statements: &mut CorporationStatements,
        record: &Value,
    ) -> Result<bool, Error> {
        let id = self.required_i64(record, "_key")?;
        let mut values = self.corporation_head_values(record, id)?;
        values.extend(self.corporation_tail_values(record)?);
        statements
            .corp
            .execute(rusqlite::params_from_iter(values))?;

        for allowed_race_id in self.optional_i64_array(record, "allowedMemberRaces")? {
            statements
                .allowed_race
                .execute(rusqlite::params![id, allowed_race_id])?;
        }
        self.insert_corporation_pairs(&mut statements.division, record, id, "divisions")?;
        self.insert_corporation_pairs(&mut statements.trade, record, id, "corporationTrades")?;
        self.insert_corporation_pairs(&mut statements.investor, record, id, "investors")?;
        Ok(true)
    }

    /// First half of the `npcCorporations` columns, in table order.
    fn corporation_head_values(&self, record: &Value, id: i64) -> Result<Vec<SqlValue>, Error> {
        Ok(vec![
            id.into(),
            self.config
                .required_localized(record, "name")?
                .to_owned()
                .into(),
            self.required_str(record, "tickerName")?.to_owned().into(),
            self.required_bool(record, "deleted")?.into(),
            self.config
                .localized(record, "description")
                .map(str::to_owned)
                .into(),
            self.required_str(record, "extent")?.to_owned().into(),
            self.required_bool(record, "hasPlayerPersonnelManager")?
                .into(),
            self.required_i64(record, "initialPrice")?.into(),
            self.required_i64(record, "memberLimit")?.into(),
        ])
    }

    /// Second half of the `npcCorporations` columns, in table order.
    fn corporation_tail_values(&self, record: &Value) -> Result<Vec<SqlValue>, Error> {
        Ok(vec![
            self.required_f64(record, "minSecurity")?.into(),
            self.required_f64(record, "minimumJoinStanding")?.into(),
            self.required_bool(record, "sendCharTerminationMessage")?
                .into(),
            self.required_i64(record, "shares")?.into(),
            self.required_str(record, "size")?.to_owned().into(),
            self.optional_f64(record, "sizeFactor").into(),
            self.required_f64(record, "taxRate")?.into(),
            self.required_bool(record, "uniqueName")?.into(),
            self.optional_i64(record, "ceoID").into(),
            self.optional_i64(record, "mainActivityID").into(),
            self.optional_i64(record, "secondaryActivityID").into(),
            self.optional_i64(record, "iconID").into(),
            self.optional_i64(record, "raceID").into(),
            self.optional_i64(record, "enemyID").into(),
            self.optional_i64(record, "friendID").into(),
            self.optional_i64(record, "factionID").into(),
            self.optional_i64(record, "solarSystemID").into(),
            self.optional_i64(record, "stationID").into(),
        ])
    }

    /// Inserts the child rows of a corporation held in the array `field`
    /// (`divisions`, `corporationTrades` or `investors`); a missing array is
    /// simply no rows.
    fn insert_corporation_pairs(
        &self,
        statement: &mut rusqlite::Statement,
        record: &Value,
        corporation_id: i64,
        field: &str,
    ) -> Result<(), Error> {
        let Some(Value::Array(entries)) = usage::field(record, field) else {
            return Ok(());
        };
        for entry in entries {
            let key = self.required_i64(entry, "_key")?;
            if field == "divisions" {
                statement.execute(rusqlite::params![
                    corporation_id,
                    key,
                    self.required_i64(entry, "divisionNumber")?,
                    self.required_i64(entry, "leaderID")?,
                    self.required_i64(entry, "size")?
                ])?;
            } else {
                statement.execute(rusqlite::params![
                    corporation_id,
                    key,
                    self.required_f64(entry, "_value")?
                ])?;
            }
        }
        Ok(())
    }

    // factions (+ factionRace)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::parser::test_support::TempSdeDir;

    #[test]
    fn parse_npc_corporations_inserts_rows() {
        let dir = TempSdeDir::new(
            "npc_corporations",
            &[(
                "npcCorporations.jsonl",
                "{\"_key\": 1000004, \"name\": {\"en\": \"CBD Corporation\"}, \
                 \"tickerName\": \"CBD\", \"deleted\": false, \"description\": {\"en\": \"A corp\"}, \
                 \"extent\": \"L\", \"hasPlayerPersonnelManager\": false, \"initialPrice\": 0, \
                 \"memberLimit\": -1, \"minSecurity\": 0.0, \"minimumJoinStanding\": 1, \
                 \"sendCharTerminationMessage\": true, \"shares\": 1000, \"size\": \"L\", \
                 \"sizeFactor\": 5.0, \"taxRate\": 0.1, \"uniqueName\": true, \"ceoID\": 3000001, \
                 \"iconID\": 500, \"raceID\": 1}\n",
            )],
        );
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        // races(1) lo exige la FK de npcCorporations.raceId.
        connection
            .execute(
                "INSERT INTO races (raceId, raceName) VALUES (1, 'Caldari')",
                [],
            )
            .unwrap();
        let config = ParserConfig::default();
        let parser = Parser::new(&dir.path, config);

        let count = parser.parse_npc_corporations(&connection).unwrap();
        assert_eq!(count, 1);

        let (name, ticker, deleted, extent, shares, ceo_id, icon_id, race_id): (
            String,
            String,
            i64,
            String,
            i64,
            i64,
            i64,
            i64,
        ) = connection
            .query_row(
                "SELECT corporationName, tickerName, deleted, extent, shares, ceoId, iconId, raceId \
                     FROM npcCorporations WHERE corporationId = 1000004",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(name, "CBD Corporation");
        assert_eq!(ticker, "CBD");
        assert_eq!(deleted, 0);
        assert_eq!(extent, "L");
        assert_eq!(shares, 1000);
        assert_eq!(ceo_id, 3000001);
        assert_eq!(icon_id, 500);
        assert_eq!(race_id, 1);
    }

    #[test]
    fn parse_npc_corporations_populates_all_four_junction_tables() {
        let dir = TempSdeDir::new(
            "npc_corp_junctions",
            &[(
                "npcCorporations.jsonl",
                "{\"_key\": 1000002, \"name\": {\"en\": \"Corp\"}, \"tickerName\": \"C\", \
                 \"deleted\": false, \"extent\": \"L\", \"hasPlayerPersonnelManager\": false, \
                 \"initialPrice\": 0, \"memberLimit\": -1, \"minSecurity\": 0.0, \
                 \"minimumJoinStanding\": 1, \"sendCharTerminationMessage\": true, \
                 \"shares\": 1000, \"size\": \"L\", \"taxRate\": 0.1, \"uniqueName\": true, \
                 \"allowedMemberRaces\": [1], \
                 \"divisions\": [{\"_key\": 22, \"divisionNumber\": 1, \"leaderID\": 3008500, \"size\": 37}], \
                 \"corporationTrades\": [{\"_key\": 41, \"_value\": 0.42}], \
                 \"investors\": [{\"_key\": 1000002, \"_value\": 42}]}\n",
            )],
        );
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        connection
            .execute(
                "INSERT INTO races (raceId, raceName) VALUES (1, 'Caldari')",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO npcCorporationDivisions (divisionId, internalName, leaderTypeName) \
                 VALUES (22, 'Distribution', 'Distribution Manager')",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO invTypes (typeId, groupId, typeName, published) VALUES (41, NULL, 'x', 0)",
                [],
            )
            .unwrap();
        let config = ParserConfig::default();
        let parser = Parser::new(&dir.path, config);

        let count = parser.parse_npc_corporations(&connection).unwrap();
        assert_eq!(count, 1);

        let allowed_race: i64 = connection
            .query_row(
                "SELECT raceId FROM npcCorporationAllowedRaces WHERE corporationId = 1000002",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(allowed_race, 1);

        let (division_number, leader_id, division_size): (i64, i64, i64) = connection
            .query_row(
                "SELECT divisionNumber, leaderId, size FROM npcCorporationDivisionAssignments \
                     WHERE corporationId = 1000002 AND divisionId = 22",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(division_number, 1);
        assert_eq!(leader_id, 3008500);
        assert_eq!(division_size, 37);

        let affinity: f64 = connection
            .query_row(
                "SELECT affinity FROM npcCorporationTrades WHERE corporationId = 1000002 AND typeId = 41",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(affinity, 0.42);

        // Auto-inversion: la propia corp aparece como su investor -- caso
        // real confirmado (corp 1000002 en los datos reales).
        let investor_shares: f64 = connection
            .query_row(
                "SELECT shares FROM npcCorporationInvestors \
                     WHERE corporationId = 1000002 AND investorId = 1000002",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(investor_shares, 42.0);
    }

    #[test]
    fn parse_npc_corporations_missing_ticker_errors() {
        // tickerName is TEXT NOT NULL and is accessed as a required field.
        let dir = TempSdeDir::new(
            "npc_corp_missing_ticker",
            &[(
                "npcCorporations.jsonl",
                "{\"_key\": 1, \"name\": {\"en\": \"Test\"}, \"deleted\": false}\n",
            )],
        );
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        let config = ParserConfig::default();
        let parser = Parser::new(&dir.path, config);

        let result = parser.parse_npc_corporations(&connection);
        assert!(result.is_err());
    }
}
