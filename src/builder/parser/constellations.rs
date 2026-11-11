//! `mapConstellations`.

use super::*;

impl Parser {
    /// Populates `mapConstellations` from
    /// `<sde_directory>/mapConstellations.jsonl`. Requires `mapRegions` to
    /// already be populated (FK `mapConstellations.regionId ->
    /// mapRegions.regionId`). Returns the number of rows inserted.
    ///
    /// The preferred id is `constellationID` if the record carries it and
    /// it's a valid integer; otherwise it falls back to `_key`.
    #[tracing::instrument]
    pub fn parse_constellations(&self, connection: &Connection) -> Result<usize, Error> {
        let mut insert_constellation = connection.prepare(
            "INSERT INTO mapConstellations (constellationId, constellationName, regionId, centerX, centerY, centerZ) \
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;

        self.for_each_record("mapConstellations", "constellations", |record| {
            let id = match self.optional_i64(record, "constellationID") {
                Some(id) => id,
                None => self.required_i64(record, "_key")?,
            };
            let name = self.config.required_localized(record, "name")?;
            let region_id = self.required_i64(record, "regionID")?;
            let (center_x, center_y, center_z) = self.required_position(record)?;

            insert_constellation.execute(rusqlite::params![
                id, name, region_id, center_x, center_y, center_z
            ])?;
            Ok(true)
        })
    }

    // mapSolarSystems
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::parser::test_support::TempSdeDir;

    #[test]
    fn parse_constellations_falls_back_to_key_when_constellation_id_absent() {
        let dir = TempSdeDir::new(
            "constellations_fallback",
            &[(
                "mapConstellations.jsonl",
                "{\"_key\": 20000020, \"name\": {\"en\": \"Kimotoro\"}, \"regionID\": 10000002, \
                 \"position\": {\"x\": 110.0, \"y\": 210.0, \"z\": 310.0}}\n",
            )],
        );
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        // mapRegions(10000002) lo exige la FK de mapConstellations.regionId.
        connection
            .execute(
                "INSERT INTO mapRegions \
                 (regionId, regionName, factionId, centerX, centerY, centerZ, nebula, wormholeClassId) \
                 VALUES (10000002, 'The Forge', NULL, 0, 0, 0, 5, NULL)",
                [],
            )
            .unwrap();
        let config = ParserConfig::default();
        let parser = Parser::new(&dir.path, config);

        let count = parser.parse_constellations(&connection).unwrap();
        assert_eq!(count, 1);

        let (id, name, region_id): (i64, String, i64) = connection
            .query_row(
                "SELECT constellationId, constellationName, regionId FROM mapConstellations",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        // No `constellationID` on the record, falls back to `_key` (20000020).
        assert_eq!(id, 20000020);
        assert_eq!(name, "Kimotoro");
        assert_eq!(region_id, 10000002);
    }

    #[test]
    fn parse_constellations_prefers_constellation_id_when_present() {
        let dir = TempSdeDir::new(
            "constellations_prefer_id",
            &[(
                "mapConstellations.jsonl",
                "{\"_key\": 999, \"constellationID\": 20000020, \"name\": {\"en\": \"Kimotoro\"}, \
                 \"regionID\": 10000002, \"position\": {\"x\": 1.0, \"y\": 2.0, \"z\": 3.0}}\n",
            )],
        );
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        connection
            .execute(
                "INSERT INTO mapRegions \
                 (regionId, regionName, factionId, centerX, centerY, centerZ, nebula, wormholeClassId) \
                 VALUES (10000002, 'The Forge', NULL, 0, 0, 0, 5, NULL)",
                [],
            )
            .unwrap();
        let config = ParserConfig::default();
        let parser = Parser::new(&dir.path, config);

        parser.parse_constellations(&connection).unwrap();

        let id: i64 = connection
            .query_row("SELECT constellationId FROM mapConstellations", [], |row| {
                row.get(0)
            })
            .unwrap();
        // constellationID (20000020) wins over _key (999).
        assert_eq!(id, 20000020);
    }
}
