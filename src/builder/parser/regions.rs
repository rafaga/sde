//! `mapRegions`.

use super::*;

impl Parser {
    /// Populates `mapRegions` from `<sde_directory>/mapRegions.jsonl`.
    /// Returns the number of rows inserted.
    ///
    /// `maxProjX`/`maxProjY` aren't included in the INSERT: the DDL gives
    /// them `DEFAULT(0.0)`, which SQLite applies automatically.
    #[tracing::instrument]
    pub fn parse_regions(&self, connection: &Connection) -> Result<usize, Error> {
        let mut insert_region = connection.prepare(
            "INSERT INTO mapRegions (regionId, regionName, factionId, centerX, centerY, centerZ, nebula, wormholeClassId) \
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;

        self.for_each_record("mapRegions", "regions", |record| {
            let id = self.required_i64(record, "_key")?;
            let name = self.config.required_localized(record, "name")?;
            let faction_id = self.optional_i64(record, "factionID");
            let nebula = self.required_i64(record, "nebulaID")?;
            let wormhole_class_id = self.optional_i64(record, "wormholeClassID");
            let (center_x, center_y, center_z) = self.required_position(record)?;

            insert_region.execute(rusqlite::params![
                id,
                name,
                faction_id,
                center_x,
                center_y,
                center_z,
                nebula,
                wormhole_class_id
            ])?;
            Ok(true)
        })
    }

    // mapConstellations
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::parser::test_support::TempSdeDir;

    #[test]
    fn parse_regions_inserts_rows_with_default_max_proj() {
        let dir = TempSdeDir::new(
            "regions",
            &[(
                "mapRegions.jsonl",
                "{\"_key\": 10000002, \"name\": {\"en\": \"The Forge\"}, \"nebulaID\": 5, \
                 \"position\": {\"x\": 100.0, \"y\": 200.0, \"z\": 300.0}}\n",
            )],
        );
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        let config = ParserConfig::default();
        let parser = Parser::new(&dir.path, config);

        let count = parser.parse_regions(&connection).unwrap();
        assert_eq!(count, 1);

        let (name, faction_id, cx, cy, cz, nebula, wh_class, max_x, max_y): (
            String,
            Option<i64>,
            f64,
            f64,
            f64,
            i64,
            Option<i64>,
            f64,
            f64,
        ) = connection
            .query_row(
                "SELECT regionName, factionId, centerX, centerY, centerZ, nebula, \
                 wormholeClassId, maxProjX, maxProjY FROM mapRegions WHERE regionId = 10000002",
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
                        row.get(8)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(name, "The Forge");
        assert_eq!(faction_id, None);
        assert_eq!((cx, cy, cz), (100.0, 200.0, 300.0));
        assert_eq!(nebula, 5);
        assert_eq!(wh_class, None);
        // maxProjX/maxProjY aren't inserted explicitly -- they should
        // come out of the DDL's DEFAULT(0.0).
        assert_eq!((max_x, max_y), (0.0, 0.0));
    }

    #[test]
    fn parse_regions_missing_nebula_errors() {
        // mapRegions.nebula is INTEGER NOT NULL -- see "Notable behavior"
        // in the module's docstring.
        let dir = TempSdeDir::new(
            "regions_missing_nebula",
            &[(
                "mapRegions.jsonl",
                "{\"_key\": 1, \"name\": {\"en\": \"Test\"}, \
                 \"position\": {\"x\": 0.0, \"y\": 0.0, \"z\": 0.0}}\n",
            )],
        );
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        let config = ParserConfig::default();
        let parser = Parser::new(&dir.path, config);

        let result = parser.parse_regions(&connection);
        assert!(result.is_err());
    }

    #[test]
    fn parse_regions_missing_position_errors() {
        let dir = TempSdeDir::new(
            "regions_missing_position",
            &[(
                "mapRegions.jsonl",
                "{\"_key\": 1, \"name\": {\"en\": \"Test\"}, \"nebulaID\": 0}\n",
            )],
        );
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        let config = ParserConfig::default();
        let parser = Parser::new(&dir.path, config);

        let result = parser.parse_regions(&connection);
        assert!(result.is_err());
    }
}
