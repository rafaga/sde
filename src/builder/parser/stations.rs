//! Station services, station operations and NPC stations.

use super::*;

impl Parser {
    /// Populates `stationServices` from `<sde_directory>/stationServices.jsonl`
    /// (27 records, confirmed complete: `_key`/`serviceName` present in
    /// 100% of records). See [`Self::parse_npc_stations`]'s docstring for
    /// why `staStation`/`staCorporations`, which used to cover this area of
    /// the schema, are gone.
    #[tracing::instrument]
    pub fn parse_station_services(&self, connection: &Connection) -> Result<usize, Error> {
        let mut insert = connection
            .prepare("INSERT INTO stationServices (serviceId, serviceName) VALUES (?1, ?2)")?;

        self.for_each_record("stationServices", "station services", |record| {
            let id = self.required_i64(record, "_key")?;
            let name = self.config.required_localized(record, "serviceName")?;
            insert.execute(rusqlite::params![id, name])?;
            Ok(true)
        })
    }

    /// Populates `stationOperations`, `stationOperationServices`, and
    /// `stationOperationTypes` from
    /// `<sde_directory>/stationOperations.jsonl` (68 records). Requires
    /// [`Self::parse_station_services`]/[`crate::builder::parser::Parser::parse_types`]
    /// to have already run -- the two junction
    /// tables reference `stationServices`/`invTypes`.
    ///
    /// Confirmed against the real 68 records: `_key`, `activityID`,
    /// `border`, `corridor`, `fringe`, `hub`, `manufacturingFactor`,
    /// `operationName`, `ratio`, `researchFactor`, `services` are present
    /// in 100% of records -- treated as required
    /// (`required_i64`/`required_f64`/`ParserConfig::required_localized`).
    /// `description` is present in 55/68 (80.9%) -- optional
    /// (`ParserConfig::localized`, not `ParserConfig::required_localized`). `stationTypes` is
    /// present in 47/68 (69.1%) -- also optional, only inserted into
    /// `stationOperationTypes` when the record actually carries it.
    ///
    /// Each `stationTypes` entry is `{"_key": <sizeKey>, "_value": <typeId>}`
    /// -- `_key` takes one of exactly 5 values across all 68 records (1, 2,
    /// 4, 8, 16, confirmed by exhaustive check), consistent with a
    /// station-size bit-flag, though the SDE itself doesn't document what
    /// each flag means beyond the raw value; `stationOperationTypes.sizeKey`
    /// is kept as a plain integer rather than guessing at named constants.
    #[tracing::instrument]
    pub fn parse_station_operations(&self, connection: &Connection) -> Result<usize, Error> {
        let mut insert_operation = connection.prepare(
            "INSERT INTO stationOperations \
            (operationId, activityId, operationName, description, border, corridor, fringe, hub, \
            ratio, manufacturingFactor, researchFactor) \
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        )?;
        let mut insert_service = connection.prepare(
            "INSERT INTO stationOperationServices (operationId, serviceId) VALUES (?1, ?2)",
        )?;
        let mut insert_type = connection.prepare(
            "INSERT INTO stationOperationTypes (operationId, sizeKey, typeId) VALUES (?1, ?2, ?3)",
        )?;

        self.for_each_record("stationOperations", "station operations", |record| {
            let id = self.required_i64(record, "_key")?;
            let values = self.station_operation_values(record, id)?;
            insert_operation.execute(rusqlite::params_from_iter(values))?;

            for service_id in self.optional_i64_array(record, "services")? {
                insert_service.execute(rusqlite::params![id, service_id])?;
            }
            self.insert_station_operation_types(&mut insert_type, record, id)?;

            Ok(true)
        })
    }

    /// The `stationOperations` columns of one record, in table order.
    fn station_operation_values(&self, record: &Value, id: i64) -> Result<Vec<SqlValue>, Error> {
        Ok(vec![
            id.into(),
            self.required_i64(record, "activityID")?.into(),
            self.config
                .required_localized(record, "operationName")?
                .to_owned()
                .into(),
            self.config
                .localized(record, "description")
                .map(str::to_owned)
                .into(),
            self.required_f64(record, "border")?.into(),
            self.required_f64(record, "corridor")?.into(),
            self.required_f64(record, "fringe")?.into(),
            self.required_f64(record, "hub")?.into(),
            self.required_f64(record, "ratio")?.into(),
            self.required_f64(record, "manufacturingFactor")?.into(),
            self.required_f64(record, "researchFactor")?.into(),
        ])
    }

    /// Inserts the `stationOperationTypes` rows of an operation; a missing
    /// `stationTypes` array is simply no rows.
    fn insert_station_operation_types(
        &self,
        statement: &mut rusqlite::Statement,
        record: &Value,
        operation_id: i64,
    ) -> Result<(), Error> {
        let Some(Value::Array(station_types)) = usage::field(record, "stationTypes") else {
            return Ok(());
        };
        for entry in station_types {
            let size_key = self.required_i64(entry, "_key")?;
            let type_id = self.required_i64(entry, "_value")?;
            statement.execute(rusqlite::params![operation_id, size_key, type_id])?;
        }
        Ok(())
    }

    /// Populates `npcStations` from `<sde_directory>/npcStations.jsonl`
    /// (5210 records). Requires `mapMoons`/`mapPlanets`,
    /// `mapSolarSystems`, `npcCorporations`, `invTypes`,
    /// and [`Self::parse_station_operations`] to have already run --
    /// every foreign key on this table points somewhere.
    ///
    /// # Why `npcStations`, not `staStation`/`staCorporations`
    ///
    /// `staStation`/`staCorporations` (an older table-naming convention)
    /// aren't in the schema at all. The real SDE export uses a different
    /// table name (`npcStations`) and a materially richer shape
    /// (reprocessing data, station operation/services, precise real-world
    /// position), so `npcStations` is built directly against that shape.
    ///
    /// # `orbitID` split into `orbitMoonId`/`orbitPlanetId`
    ///
    /// The real SDE's `orbitID` can be either a moon or a planet --
    /// confirmed by cross-referencing all 5210 real `orbitID` values
    /// against real `mapMoons`/`mapPlanets` samples: 76.5% are moons,
    /// 23.5% are planets, and exactly 1 (a singular, special station whose
    /// `orbitID` matches neither) is neither. SQL can't express a single
    /// foreign key conditional on two different target tables, so the
    /// schema splits this into two mutually-exclusive nullable columns
    /// instead -- resolved here at parse time by checking membership
    /// against in-memory sets of every already-inserted `moonId`/`planetId`
    /// (both empty only in that one singular case, in which case both
    /// columns stay `NULL`).
    ///
    /// `celestialIndex` (present in 5209/5210, 99.98%) and `orbitIndex`
    /// (present in 3986/5210, 76.5% -- exactly the stations that orbit a
    /// moon) are both treated as optional (`optional_i64`), matching
    /// their real, confirmed absence rate -- not just a defensive
    /// assumption.
    #[tracing::instrument]
    pub fn parse_npc_stations(&self, connection: &Connection) -> Result<usize, Error> {
        let moon_ids = Self::load_id_set(connection, "SELECT moonId FROM mapMoons")?;
        let planet_ids = Self::load_id_set(connection, "SELECT planetId FROM mapPlanets")?;

        let mut insert = connection.prepare(
            "INSERT INTO npcStations \
            (stationId, celestialIndex, operationId, orbitMoonId, orbitPlanetId, orbitIndex, \
            ownerId, positionX, positionY, positionZ, reprocessingEfficiency, \
            reprocessingHangarFlag, reprocessingStationsTake, solarSystemId, typeId, \
            useOperationName) \
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
        )?;

        self.for_each_record("npcStations", "NPC stations", |record| {
            let id = self.required_i64(record, "_key")?;
            let values = self.npc_station_values(record, id, &moon_ids, &planet_ids)?;
            insert.execute(rusqlite::params_from_iter(values))?;
            Ok(true)
        })
    }

    /// Every id returned by `sql` (a single-column query), as a set.
    fn load_id_set(
        connection: &Connection,
        sql: &str,
    ) -> Result<std::collections::HashSet<i64>, Error> {
        let mut statement = connection.prepare(sql)?;
        let mut rows = statement.query([])?;
        let mut ids = std::collections::HashSet::new();
        while let Some(row) = rows.next()? {
            ids.insert(row.get(0)?);
        }
        Ok(ids)
    }

    /// Splits an `orbitID` into the `(orbitMoonId, orbitPlanetId)` pair by
    /// checking which of the already-inserted moons/planets it belongs to.
    fn orbit_columns(
        orbit_id: i64,
        moon_ids: &std::collections::HashSet<i64>,
        planet_ids: &std::collections::HashSet<i64>,
    ) -> (Option<i64>, Option<i64>) {
        if moon_ids.contains(&orbit_id) {
            (Some(orbit_id), None)
        } else if planet_ids.contains(&orbit_id) {
            (None, Some(orbit_id))
        } else {
            (None, None)
        }
    }

    /// The `npcStations` columns of one record, in table order.
    fn npc_station_values(
        &self,
        record: &Value,
        id: i64,
        moon_ids: &std::collections::HashSet<i64>,
        planet_ids: &std::collections::HashSet<i64>,
    ) -> Result<Vec<SqlValue>, Error> {
        let celestial_index = self.optional_i64(record, "celestialIndex");
        let operation_id = self.required_i64(record, "operationID")?;
        let orbit_id = self.required_i64(record, "orbitID")?;
        let (orbit_moon_id, orbit_planet_id) = Self::orbit_columns(orbit_id, moon_ids, planet_ids);
        let (x, y, z) = self.required_position(record)?;
        Ok(vec![
            id.into(),
            celestial_index.into(),
            operation_id.into(),
            orbit_moon_id.into(),
            orbit_planet_id.into(),
            self.optional_i64(record, "orbitIndex").into(),
            self.required_i64(record, "ownerID")?.into(),
            x.into(),
            y.into(),
            z.into(),
            self.required_f64(record, "reprocessingEfficiency")?.into(),
            self.required_i64(record, "reprocessingHangarFlag")?.into(),
            self.required_f64(record, "reprocessingStationsTake")?
                .into(),
            self.required_i64(record, "solarSystemID")?.into(),
            self.required_i64(record, "typeID")?.into(),
            self.required_bool(record, "useOperationName")?.into(),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::parser::test_support::TempSdeDir;

    #[test]
    fn parse_station_services_inserts_rows() {
        let dir = TempSdeDir::new(
            "station_services",
            &[(
                "stationServices.jsonl",
                "{\"_key\": 3, \"serviceName\": {\"en\": \"Courier Missions\"}}\n\
                 {\"_key\": 5, \"serviceName\": {\"en\": \"Reprocessing Plant\"}}\n",
            )],
        );
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        let config = ParserConfig::default();
        let parser = Parser::new(&dir.path, config);

        let count = parser.parse_station_services(&connection).unwrap();
        assert_eq!(count, 2);

        let name: String = connection
            .query_row(
                "SELECT serviceName FROM stationServices WHERE serviceId = 3",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(name, "Courier Missions");
    }

    /// Common prerequisites for `parse_station_operations`'s tests: a
    /// minimal `invTypes` row (for `stationOperationTypes.typeId`'s FK)
    /// and one `stationServices` row (for `stationOperationServices`'s
    /// FK).
    fn setup_for_station_operations(connection: &Connection) {
        crate::builder::schema::create_schema(connection).unwrap();
        connection
            .execute(
                "INSERT INTO invCategories (categoryId, categoryName, published) VALUES (1, 'Celestial', 1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO invGroups (groupId, categoryId, groupName, anchorable) VALUES (1, 1, 'Station', 0)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO invTypes (typeId, groupId, typeName, published) VALUES (1531, 1, 'Station Type', 1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO stationServices (serviceId, serviceName) VALUES (3, 'Courier Missions')",
                [],
            )
            .unwrap();
    }

    #[test]
    fn parse_station_operations_inserts_row_and_junction_tables() {
        let dir = TempSdeDir::new(
            "station_operations_full",
            &[(
                "stationOperations.jsonl",
                "{\"_key\": 26, \"activityID\": 1, \"operationName\": {\"en\": \"Test Op\"}, \
                 \"description\": {\"en\": \"A test operation\"}, \
                 \"border\": 0.0, \"corridor\": 0.2, \"fringe\": 0.7, \"hub\": 0.1, \"ratio\": 0.65, \
                 \"manufacturingFactor\": 0.98, \"researchFactor\": 0.98, \
                 \"services\": [3], \
                 \"stationTypes\": [{\"_key\": 1, \"_value\": 1531}]}\n",
            )],
        );
        let connection = Connection::open_in_memory().unwrap();
        setup_for_station_operations(&connection);
        let config = ParserConfig::default();
        let parser = Parser::new(&dir.path, config);

        let count = parser.parse_station_operations(&connection).unwrap();
        assert_eq!(count, 1);

        let name: String = connection
            .query_row(
                "SELECT operationName FROM stationOperations WHERE operationId = 26",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(name, "Test Op");

        let service_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM stationOperationServices WHERE operationId = 26",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(service_count, 1);

        let type_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM stationOperationTypes WHERE operationId = 26",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(type_count, 1);
    }

    #[test]
    fn parse_station_operations_without_description_or_station_types() {
        // Matches the real data: description present in 55/68,
        // stationTypes in 47/68 -- both genuinely optional.
        let dir = TempSdeDir::new(
            "station_operations_minimal",
            &[(
                "stationOperations.jsonl",
                "{\"_key\": 27, \"activityID\": 1, \"operationName\": {\"en\": \"Minimal Op\"}, \
                 \"border\": 0.0, \"corridor\": 0.0, \"fringe\": 0.0, \"hub\": 0.0, \"ratio\": 0.0, \
                 \"manufacturingFactor\": 0.98, \"researchFactor\": 0.98, \"services\": []}\n",
            )],
        );
        let connection = Connection::open_in_memory().unwrap();
        setup_for_station_operations(&connection);
        let config = ParserConfig::default();
        let parser = Parser::new(&dir.path, config);

        let count = parser.parse_station_operations(&connection).unwrap();
        assert_eq!(count, 1);

        let description: Option<String> = connection
            .query_row(
                "SELECT description FROM stationOperations WHERE operationId = 27",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(description, None);

        let type_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM stationOperationTypes WHERE operationId = 27",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(type_count, 0);
    }

    /// Common prerequisites for `parse_npc_stations`'s tests: everything
    /// `setup_for_station_operations` provides, plus a solar system, a
    /// planet (40000001), a moon orbiting that planet (40000002), a
    /// corporation (1000002), and a `stationOperations` row (26) --
    /// enough to satisfy every foreign key `npcStations` declares.
    fn setup_for_npc_stations(connection: &Connection) {
        setup_for_station_operations(connection);
        connection
            .execute(
                "INSERT INTO races (raceId, raceName) VALUES (1, 'Caldari')",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO npcCorporations \
                 (corporationId, corporationName, tickerName, deleted, extent, \
                  hasPlayerPersonnelManager, initialPrice, memberLimit, minSecurity, \
                  minimumJoinStanding, sendCharTerminationMessage, shares, size, taxRate, \
                  uniqueName, raceId) \
                 VALUES (1000002, 'Test Corp', 'TEST', 0, 'L', 0, 0, -1, 0.0, 1, 1, 1000, \
                          'L', 0.0, 1, 1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO mapRegions (regionId, regionName, factionId, centerX, centerY, centerZ, nebula, wormholeClassId) \
                 VALUES (10000002, 'The Forge', NULL, 0, 0, 0, 5, NULL)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO mapConstellations (constellationId, constellationName, regionId, centerX, centerY, centerZ) \
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
                 VALUES (40000001, 30000001, 1, 1531, 0, 0, 0)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO mapMoons \
                 (solarSystemId, moonId, moonIndex, planetId, typeId, positionX, positionY, positionZ) \
                 VALUES (30000001, 40000002, 1, 40000001, 1531, 0, 0, 0)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO stationOperations \
                 (operationId, activityId, operationName, border, corridor, fringe, hub, ratio, \
                  manufacturingFactor, researchFactor) \
                 VALUES (26, 1, 'Test Op', 0.0, 0.2, 0.7, 0.1, 0.65, 0.98, 0.98)",
                [],
            )
            .unwrap();
    }

    #[test]
    fn parse_npc_stations_resolves_moon_orbit() {
        let dir = TempSdeDir::new(
            "npc_stations_moon",
            &[(
                "npcStations.jsonl",
                "{\"_key\": 60000004, \"celestialIndex\": 10, \"operationID\": 26, \
                 \"orbitID\": 40000002, \"orbitIndex\": 1, \"ownerID\": 1000002, \
                 \"position\": {\"x\": 1.0, \"y\": 2.0, \"z\": 3.0}, \
                 \"reprocessingEfficiency\": 0.5, \"reprocessingHangarFlag\": 4, \
                 \"reprocessingStationsTake\": 0.05, \"solarSystemID\": 30000001, \
                 \"typeID\": 1531, \"useOperationName\": true}\n",
            )],
        );
        let connection = Connection::open_in_memory().unwrap();
        setup_for_npc_stations(&connection);

        let config = ParserConfig::default();
        let parser = Parser::new(&dir.path, config);
        let count = parser.parse_npc_stations(&connection).unwrap();
        assert_eq!(count, 1);

        let (orbit_moon, orbit_planet): (Option<i64>, Option<i64>) = connection
            .query_row(
                "SELECT orbitMoonId, orbitPlanetId FROM npcStations WHERE stationId = 60000004",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(orbit_moon, Some(40000002));
        assert_eq!(orbit_planet, None);
    }

    #[test]
    fn parse_npc_stations_resolves_planet_orbit() {
        let dir = TempSdeDir::new(
            "npc_stations_planet",
            &[(
                "npcStations.jsonl",
                "{\"_key\": 60000010, \"celestialIndex\": 1, \"operationID\": 26, \
                 \"orbitID\": 40000001, \"ownerID\": 1000002, \
                 \"position\": {\"x\": 1.0, \"y\": 2.0, \"z\": 3.0}, \
                 \"reprocessingEfficiency\": 0.5, \"reprocessingHangarFlag\": 4, \
                 \"reprocessingStationsTake\": 0.05, \"solarSystemID\": 30000001, \
                 \"typeID\": 1531, \"useOperationName\": true}\n",
            )],
        );
        let connection = Connection::open_in_memory().unwrap();
        setup_for_npc_stations(&connection);

        let config = ParserConfig::default();
        let parser = Parser::new(&dir.path, config);
        let count = parser.parse_npc_stations(&connection).unwrap();
        assert_eq!(count, 1);

        let (orbit_moon, orbit_planet, orbit_index): (Option<i64>, Option<i64>, Option<i64>) = connection
            .query_row(
                "SELECT orbitMoonId, orbitPlanetId, orbitIndex FROM npcStations WHERE stationId = 60000010",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(orbit_moon, None);
        assert_eq!(orbit_planet, Some(40000001));
        // orbitIndex genuinely absent in this fixture, matching the real
        // pattern (only present for moon-orbiting stations).
        assert_eq!(orbit_index, None);
    }

    #[test]
    fn parse_npc_stations_leaves_both_orbit_columns_null_when_neither_matches() {
        // Mirrors the one real record (60015187) whose orbitID matches
        // neither a real moon nor a real planet.
        let dir = TempSdeDir::new(
            "npc_stations_neither",
            &[(
                "npcStations.jsonl",
                "{\"_key\": 60015187, \"operationID\": 26, \
                 \"orbitID\": 999999999, \"ownerID\": 1000002, \
                 \"position\": {\"x\": 1.0, \"y\": 2.0, \"z\": 3.0}, \
                 \"reprocessingEfficiency\": 0.5, \"reprocessingHangarFlag\": 4, \
                 \"reprocessingStationsTake\": 0.025, \"solarSystemID\": 30000001, \
                 \"typeID\": 1531, \"useOperationName\": true}\n",
            )],
        );
        let connection = Connection::open_in_memory().unwrap();
        setup_for_npc_stations(&connection);

        let config = ParserConfig::default();
        let parser = Parser::new(&dir.path, config);
        let count = parser.parse_npc_stations(&connection).unwrap();
        assert_eq!(count, 1);

        let (celestial_index, orbit_moon, orbit_planet): (Option<i64>, Option<i64>, Option<i64>) = connection
            .query_row(
                "SELECT celestialIndex, orbitMoonId, orbitPlanetId FROM npcStations WHERE stationId = 60015187",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(celestial_index, None);
        assert_eq!(orbit_moon, None);
        assert_eq!(orbit_planet, None);
    }
}
