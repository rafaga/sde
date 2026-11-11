//! `races`.

use super::*;

impl Parser {
    /// Populates `races` from `<sde_directory>/races.jsonl`. Returns the
    /// number of rows inserted.
    #[tracing::instrument]
    pub fn parse_races(&self, connection: &Connection) -> Result<usize, Error> {
        let mut insert_race =
            connection.prepare("INSERT INTO races (raceId, raceName) VALUES (?1, ?2)")?;

        self.for_each_record("races", "races", |record| {
            let id = self.required_i64(record, "_key")?;
            let name = self.config.required_localized(record, "name")?;

            insert_race.execute(rusqlite::params![id, name])?;
            Ok(true)
        })
    }

    // npcCorporationDivisions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::parser::test_support::TempSdeDir;

    #[test]
    fn parse_races_inserts_rows() {
        let dir = TempSdeDir::new(
            "races",
            &[(
                "races.jsonl",
                "{\"_key\": 1, \"name\": {\"en\": \"Caldari\"}}\n\
                 {\"_key\": 2, \"name\": {\"en\": \"Minmatar\"}}\n",
            )],
        );
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        let config = ParserConfig::default();
        let parser = Parser::new(&dir.path, config);

        let count = parser.parse_races(&connection).unwrap();
        assert_eq!(count, 2);

        let name: String = connection
            .query_row("SELECT raceName FROM races WHERE raceId = 1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(name, "Caldari");
    }
}
