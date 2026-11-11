//! `invTypes`, `invCategories` and `invGroups`, and the star types (`typeStar`) found among the types.

use super::*;

/// State shared between [`Parser::parse_groups`] and [`Parser::parse_types`].
#[derive(Debug, Default)]
pub struct StarTypeState {
    /// `groupId` of the group named exactly `"Sun"`, once
    /// [`Parser::parse_groups`] finds it.
    pub sun_group_id: Option<i64>,
    /// Every `typeId` (from `invTypes`) that [`Parser::parse_types`]
    /// inserted into `typeStar`. Used by [`Parser::parse_stars`] to
    /// validate a star's `typeID` before writing it directly as
    /// `mapStars.starTypeId` -- `typeStar.typeId` is `mapStars`'
    /// `starTypeId`'s FK target (`typeStar`'s primary key *is*
    /// `typeId`; there's no separate, self-assigned id to translate
    /// into), so this only needs to say "is this typeId one of them",
    /// not map it to anything else.
    pub star_type_ids: std::collections::HashSet<i64>,
}

/// Root of `star_colors.json`, embedded via `include_str!`: maps each
/// spectral class (e.g. `"G5"`) to its color info.
#[derive(Debug, serde::Deserialize)]
struct StarColors {
    spectral_classes: std::collections::HashMap<String, StarColorEntry>,
}

/// A single spectral class entry. Only `hex` is consumed (the RGB code
/// stored in `typeStar.color`); `temperature_K`/`rgb` are ignored.
#[derive(Debug, serde::Deserialize)]
struct StarColorEntry {
    hex: String,
}

// ---------------------------------------------------------------------
// Shared infrastructure: flat-file reading + field-extraction helpers.
// ---------------------------------------------------------------------

impl Parser {
    /// Inserts a row into `typeStar`, keyed by `type_id` directly --
    /// `typeStar.typeId` is its own primary key (a genuine 1:1 with
    /// `invTypes`, confirmed by every real "Sun"-group type having a
    /// unique `_key`/`typeId`), not a separate, self-assigned id, so
    /// there's nothing to read back after the `INSERT`.
    fn add_star_type(
        &self,
        connection: &Connection,
        type_id: i64,
        name: &str,
        color: &str,
    ) -> Result<(), Error> {
        connection.execute(
            "INSERT INTO typeStar (typeId, name, color) VALUES (?1, ?2, ?3)",
            rusqlite::params![type_id, name, color],
        )?;
        Ok(())
    }

    /// Populates `invTypes` from `<sde_directory>/types.jsonl`, and along
    /// the way `typeStar` for any type belonging to the "Sun" group (detected
    /// by [`Self::parse_groups`] via `state.sun_group_id`). Returns the number of
    /// rows inserted into `invTypes`. See "Notable behavior" in the
    /// module's docstring for how malformed star names are handled.
    #[tracing::instrument(skip(state))]
    pub fn parse_types(
        &self,
        connection: &Connection,
        state: &mut StarTypeState,
    ) -> Result<usize, Error> {
        let mut insert_type = connection.prepare(
            "INSERT INTO invTypes (typeId, groupId, typeName, iconId, published, volume) \
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;

        let star_colors: StarColors = serde_json::from_str(include_str!("../star_colors.json"))?;

        self.for_each_record("types", "types", |record| {
            let id = self.required_i64(record, "_key")?;
            let group_id = self.required_i64(record, "groupID")?;
            let name = self.config.required_localized(record, "name")?.to_string();
            let icon_id = self.optional_i64(record, "iconID");
            let published = self.optional_bool(record, "published");
            let volume = self.optional_f64(record, "volume");

            insert_type.execute(rusqlite::params![
                id, group_id, name, icon_id, published, volume
            ])?;

            if state.sun_group_id == Some(group_id) {
                let parts: Vec<&str> = name.split(' ').collect();
                if parts.len() >= 3 {
                    let star_name = parts[1];
                    let spectral_class: String = star_name.chars().take(2).collect();
                    let color = &star_colors
                        .spectral_classes
                        .get(&spectral_class)
                        .ok_or_else(|| {
                            Error::data(format!(
                                "unknown spectral class `{spectral_class}` (from `{star_name}`) \
                                 in star_colors.json"
                            ))
                        })?
                        .hex;
                    self.add_star_type(connection, id, star_name, color)?;
                    state.star_type_ids.insert(id);
                }
                // Fewer than 3 tokens: not treated as a star. See
                // "Notable behavior" in the module's docstring.
            }

            Ok(true)
        })
    }

    // invCategories

    /// Populates `invCategories` from `<sde_directory>/categories.jsonl`.
    /// Returns the number of rows inserted.
    #[tracing::instrument]
    pub fn parse_categories(&self, connection: &Connection) -> Result<usize, Error> {
        let mut insert_category = connection.prepare(
            "INSERT INTO invCategories (categoryId, categoryName, published) VALUES (?1, ?2, ?3)",
        )?;

        self.for_each_record("categories", "categories", |record| {
            let id = self.required_i64(record, "_key")?;
            let name = self.config.required_localized(record, "name")?;
            let published = self.optional_bool(record, "published");

            insert_category.execute(rusqlite::params![id, name, published])?;
            Ok(true)
        })
    }

    // invGroups

    /// Populates `invGroups` from `<sde_directory>/groups.jsonl`. Along the
    /// way, detects the group named exactly `"Sun"` and saves its id in
    /// `state.sun_group_id` -- [`Self::parse_types`] needs it to recognize star
    /// types. Returns the number of rows inserted.
    #[tracing::instrument(skip(state))]
    pub fn parse_groups(
        &self,
        connection: &Connection,
        state: &mut StarTypeState,
    ) -> Result<usize, Error> {
        let mut insert_group = connection.prepare(
            "INSERT INTO invGroups (groupId, categoryId, groupName, anchorable) \
            VALUES (?1, ?2, ?3, ?4)",
        )?;

        self.for_each_record("groups", "groups", |record| {
            let id = self.required_i64(record, "_key")?;
            let category_id = self.required_i64(record, "categoryID")?;
            let name = self.config.required_localized(record, "name")?;
            let anchorable = self.optional_bool(record, "anchorable");

            insert_group.execute(rusqlite::params![id, category_id, name, anchorable])?;

            if name == "Sun" {
                state.sun_group_id = Some(id);
            }

            Ok(true)
        })
    }

    // races
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::parser::test_support::TempSdeDir;

    #[test]
    fn parse_categories_inserts_rows() {
        let dir = TempSdeDir::new(
            "categories",
            &[(
                "categories.jsonl",
                "{\"_key\": 6, \"name\": {\"en\": \"Ship\"}, \"published\": true}\n",
            )],
        );
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        let config = ParserConfig::default();
        let parser = Parser::new(&dir.path, config);

        let count = parser.parse_categories(&connection).unwrap();
        assert_eq!(count, 1);

        let (name, published): (String, i64) = connection
            .query_row(
                "SELECT categoryName, published FROM invCategories WHERE categoryId = 6",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(name, "Ship");
        assert_eq!(published, 1);
    }

    #[test]
    fn parse_groups_and_types_detect_sun_and_populate_typestar() {
        let dir = TempSdeDir::new(
            "groups_types",
            &[
                (
                    "groups.jsonl",
                    "{\"_key\": 6, \"categoryID\": 6, \"name\": {\"en\": \"Sun\"}, \"anchorable\": false}\n\
                     {\"_key\": 7, \"categoryID\": 6, \"name\": {\"en\": \"Frigate\"}, \"anchorable\": false}\n",
                ),
                (
                    "types.jsonl",
                    "{\"_key\": 3000, \"groupID\": 6, \"name\": {\"en\": \"Yellow G5 (ffcc00)\"}, \"iconID\": 100, \"published\": true, \"volume\": 0.0}\n\
                     {\"_key\": 588, \"groupID\": 7, \"name\": {\"en\": \"Rifter\"}, \"iconID\": 200, \"published\": true, \"volume\": 27289.5}\n",
                ),
            ],
        );
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        // invCategories(6) lo exige la FK de invGroups.categoryId.
        connection
            .execute(
                "INSERT INTO invCategories (categoryId, categoryName, published) \
                 VALUES (6, 'Celestial', 1)",
                [],
            )
            .unwrap();
        let config = ParserConfig::default();
        let parser = Parser::new(&dir.path, config);
        let mut state = StarTypeState::default();

        let groups = parser.parse_groups(&connection, &mut state).unwrap();
        assert_eq!(groups, 2);
        assert_eq!(state.sun_group_id, Some(6));

        let types = parser.parse_types(&connection, &mut state).unwrap();
        assert_eq!(types, 2);

        // The "Sun"-group type should have generated a row in typeStar.
        assert_eq!(state.star_type_ids.len(), 1);
        assert!(state.star_type_ids.contains(&3000));
        let (name, color): (String, String) = connection
            .query_row(
                "SELECT name, color FROM typeStar WHERE typeId = ?1",
                rusqlite::params![3000],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(name, "G5");
        assert_eq!(color, "#FFE996");

        // "Rifter" (Frigate group, not Sun) shouldn't generate a row in typeStar.
        let total_star_types: i64 = connection
            .query_row("SELECT COUNT(*) FROM typeStar", [], |row| row.get(0))
            .unwrap();
        assert_eq!(total_star_types, 1);
    }

    #[test]
    fn parse_categories_missing_required_key_errors() {
        let dir = TempSdeDir::new(
            "missing_key",
            &[("categories.jsonl", "{\"name\": {\"en\": \"Ship\"}}\n")],
        );
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        let config = ParserConfig::default();
        let parser = Parser::new(&dir.path, config);

        let result = parser.parse_categories(&connection);
        assert!(result.is_err());
    }

    #[test]
    fn parse_categories_missing_name_errors() {
        // categoryName is TEXT NOT NULL in the STRICT schema -- see
        // `required_localized`'s docstring.
        let dir = TempSdeDir::new("missing_name", &[("categories.jsonl", "{\"_key\": 6}\n")]);
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        let config = ParserConfig::default();
        let parser = Parser::new(&dir.path, config);

        let result = parser.parse_categories(&connection);
        assert!(result.is_err());
    }

    #[test]
    fn parse_categories_missing_file_errors() {
        let dir = TempSdeDir::new("missing_file", &[]);
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        let config = ParserConfig::default();
        let parser = Parser::new(&dir.path, config);

        // categories.jsonl was never written at all.
        let result = parser.parse_categories(&connection);
        assert!(result.is_err());
    }
}
