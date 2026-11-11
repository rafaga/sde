//! `factions` and `factionRace`.

use super::*;

impl Parser {
    /// Populates `factions` and `factionRace` from
    /// `<sde_directory>/factions.jsonl`. Requires `npcCorporations` to
    /// already be populated if any record carries `corporationID`/
    /// `militiaCorporationID`, and `races` for any id in `memberRaces`.
    /// `solarSystemId` is `DEFERRABLE` (`mapSolarSystems` is parsed later
    /// in the pipeline, after `factions`). Returns the number of factions
    /// inserted (doesn't count `factionRace` rows).
    ///
    /// Rewritten against a real 27-record sample (August 2026) --
    /// `description`/`solarSystemID` are new fields, both present in 100%
    /// of real records but not previously captured at all.
    /// `shortDescription`/`flatLogo`/`flatLogoWithName`/
    /// `militiaCorporationID` are rarer (14.8%/66.7%/22.2%/22.2%) but
    /// genuinely present.
    #[tracing::instrument]
    pub fn parse_factions(&self, connection: &Connection) -> Result<usize, Error> {
        let mut insert_faction = connection.prepare(
            "INSERT INTO factions \
            (factionId, factionName, iconId, sizeFactor, uniqueName, description, shortDescription, \
            flatLogo, flatLogoWithName, corporationId, militiaCorporationId, solarSystemId) \
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        )?;
        let mut insert_faction_race =
            connection.prepare("INSERT INTO factionRace (factionId, raceId) VALUES (?1, ?2)")?;

        self.for_each_record("factions", "factions", |record| {
            let id = self.required_i64(record, "_key")?;
            let name = self.config.required_localized(record, "name")?;
            let icon_id = self.required_i64(record, "iconID")?;
            let size_factor = self.required_f64(record, "sizeFactor")?;
            let unique_name = self.required_bool(record, "uniqueName")?;
            let description = self.config.required_localized(record, "description")?;
            let short_description = self.config.localized(record, "shortDescription");
            let flat_logo = self.optional_str(record, "flatLogo");
            let flat_logo_with_name = self.optional_str(record, "flatLogoWithName");
            let corporation_id = self.optional_i64(record, "corporationID");
            let militia_corporation_id = self.optional_i64(record, "militiaCorporationID");
            let solar_system_id = self.optional_i64(record, "solarSystemID");
            let member_races = self.optional_i64_array(record, "memberRaces")?;

            insert_faction.execute(rusqlite::params![
                id,
                name,
                icon_id,
                size_factor,
                unique_name,
                description,
                short_description,
                flat_logo,
                flat_logo_with_name,
                corporation_id,
                militia_corporation_id,
                solar_system_id
            ])?;

            for race_id in member_races {
                insert_faction_race.execute(rusqlite::params![id, race_id])?;
            }

            Ok(true)
        })
    }

    // mapRegions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::parser::test_support::TempSdeDir;

    #[test]
    fn parse_factions_inserts_faction_and_member_races() {
        let dir = TempSdeDir::new(
            "factions",
            &[(
                "factions.jsonl",
                "{\"_key\": 500001, \"name\": {\"en\": \"Caldari State\"}, \"iconID\": 600, \
                 \"sizeFactor\": 3.0, \"uniqueName\": true, \"description\": {\"en\": \"A state\"}, \
                 \"corporationID\": 1000004, \"solarSystemID\": 30002780, \"memberRaces\": [1]}\n",
            )],
        );
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        // FK prerequisites: races(1) for factionRace, npcCorporations(1000004)
        // for factions.corporationId, mapSolarSystems(30002780) for
        // factions.solarSystemId. Despite being DEFERRABLE, this test calls
        // parse_factions() directly (autocommit, no explicit transaction) --
        // each INSERT is its own implicit transaction, so the deferred check
        // still runs immediately, same trap as parse_stargates()'s mutual
        // self-reference without an explicit BEGIN/COMMIT.
        connection
            .execute(
                "INSERT INTO races (raceId, raceName) VALUES (1, 'Caldari')",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO mapRegions (regionId, regionName, factionId, centerX, centerY, centerZ, nebula, wormholeClassId) \
                 VALUES (10000064, 'R', NULL, 0, 0, 0, 5, NULL)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO mapConstellations (constellationId, constellationName, regionId, centerX, centerY, centerZ) \
                 VALUES (20000064, 'C', 10000064, 0, 0, 0)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO mapSolarSystems (solarSystemId, solarSystemName, constellationId, radius, centerX, centerY, centerZ, security) \
                 VALUES (30002780, 'S', 20000064, 1.0, 0, 0, 0, 0.5)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO npcCorporations \
                 (corporationId, corporationName, tickerName, deleted, extent, \
                  hasPlayerPersonnelManager, initialPrice, memberLimit, minSecurity, \
                  minimumJoinStanding, sendCharTerminationMessage, shares, size, taxRate, \
                  uniqueName, iconId, raceId) \
                 VALUES (1000004, 'CBD Corporation', 'CBD', 0, 'L', 0, 0, -1, 0.0, 1, 1, 1000, \
                          'L', 0.1, 1, 500, 1)",
                [],
            )
            .unwrap();
        let config = ParserConfig::default();
        let parser = Parser::new(&dir.path, config);

        let count = parser.parse_factions(&connection).unwrap();
        assert_eq!(count, 1);

        let (name, icon_id, size_factor, unique_name, corporation_id, solar_system_id): (
            String,
            i64,
            f64,
            i64,
            i64,
            i64,
        ) = connection
            .query_row(
                "SELECT factionName, iconId, sizeFactor, uniqueName, corporationId, solarSystemId \
                 FROM factions WHERE factionId = 500001",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(name, "Caldari State");
        assert_eq!(icon_id, 600);
        assert_eq!(size_factor, 3.0);
        assert_eq!(unique_name, 1);
        assert_eq!(corporation_id, 1000004);
        assert_eq!(solar_system_id, 30002780);

        let member_race: i64 = connection
            .query_row(
                "SELECT raceId FROM factionRace WHERE factionId = 500001",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(member_race, 1);
    }

    #[test]
    fn parse_factions_without_member_races_inserts_faction_only() {
        // memberRaces absent -> factionRace stays empty for this
        // faction, no error (equivalent to `faction.get('memberRaces', [])`).
        let dir = TempSdeDir::new(
            "factions_no_members",
            &[(
                "factions.jsonl",
                "{\"_key\": 500002, \"name\": {\"en\": \"Minmatar Republic\"}, \"iconID\": 601, \
                 \"sizeFactor\": 2.5, \"uniqueName\": true, \"description\": {\"en\": \"x\"}}\n",
            )],
        );
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        let config = ParserConfig::default();
        let parser = Parser::new(&dir.path, config);

        let count = parser.parse_factions(&connection).unwrap();
        assert_eq!(count, 1);

        let total_faction_race: i64 = connection
            .query_row("SELECT COUNT(*) FROM factionRace", [], |row| row.get(0))
            .unwrap();
        assert_eq!(total_faction_race, 0);
    }

    #[test]
    fn parse_factions_with_non_integer_member_race_errors() {
        let dir = TempSdeDir::new(
            "factions_bad_members",
            &[(
                "factions.jsonl",
                "{\"_key\": 500003, \"name\": {\"en\": \"Bad Faction\"}, \"iconID\": 602, \
                 \"sizeFactor\": 1.0, \"uniqueName\": false, \"description\": {\"en\": \"x\"}, \
                 \"memberRaces\": [1, \"oops\"]}\n",
            )],
        );
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        let config = ParserConfig::default();
        let parser = Parser::new(&dir.path, config);

        let result = parser.parse_factions(&connection);
        assert!(result.is_err());
    }

    #[test]
    fn parse_factions_missing_size_factor_errors() {
        // sizeFactor is REAL NOT NULL and is accessed as a required field.
        let dir = TempSdeDir::new(
            "factions_missing_size_factor",
            &[(
                "factions.jsonl",
                "{\"_key\": 1, \"name\": {\"en\": \"Test\"}, \"iconID\": 1, \"uniqueName\": true}\n",
            )],
        );
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        let config = ParserConfig::default();
        let parser = Parser::new(&dir.path, config);

        let result = parser.parse_factions(&connection);
        assert!(result.is_err());
    }
}
