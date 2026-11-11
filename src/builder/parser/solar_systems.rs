//! `mapSolarSystems` (the scope filter and the 2D projection) and its sub tables.

use super::*;

/// 2D isometric projection of a 3D point, collapsing `axis`. Returns
/// the two non-null components directly as `(x2d, y2d)`.
///
/// Formulas (from <https://www.compuphase.com/axometr.htm>):
/// - Z axis collapsed: `(x - z, y + (x + z) / 2)`
/// - Y axis collapsed: `(x - y, z + (x + y) / 2)`
/// - X axis collapsed: `(y - x, z + (y + x) / 2)`
pub fn isometric_projection_2d(x: f64, y: f64, z: f64, axis: ProjectedAxis) -> (f64, f64) {
    match axis {
        ProjectedAxis::Z => (x - z, y + (x + z) / 2.0),
        ProjectedAxis::Y => (x - y, z + (x + y) / 2.0),
        ProjectedAxis::X => (y - x, z + (y + x) / 2.0),
    }
}

/// 2D orthographic projection of a 3D point that simply drops `axis`'s
/// component, keeping the other two as-is -- the same drop semantics
/// [`crate::objects::SdePoint::to_2d`] uses on the read side. Returns
/// `(x2d, y2d)`.
///
/// - Z axis dropped: `(x, y)`
/// - Y axis dropped: `(x, z)` -- the "north-up top-down": east = +x =
///   screen right, north = +z = screen up, matching the
///   community-canonical map orientation (EVE's galactic plane is the
///   X-Z plane; `y` is the vertical axis)
/// - X axis dropped: `(y, z)`
///
/// Unlike CCP's own precomputed `position2D` (a hand-adjusted
/// schematic, k-space only), this is a true projection of the real 3D
/// coordinates, so it covers every system in scope.
pub fn orthogonal_projection_2d(x: f64, y: f64, z: f64, axis: ProjectedAxis) -> (f64, f64) {
    match axis {
        ProjectedAxis::Z => (x, y),
        ProjectedAxis::Y => (x, z),
        ProjectedAxis::X => (y, z),
    }
}

/// Solar system ids that passed the `ParserConfig::system_in_scope`
/// filter, populated by [`Parser::parse_solar_systems`]. Used by
/// [`Parser::parse_stargates`], [`Parser::parse_stars`],
/// [`Parser::parse_planets`] and [`Parser::parse_moons`] to filter
/// their own records by `solarSystemID`.
#[derive(Debug, Default)]
pub struct SystemScopeState {
    pub systems_in_scope: std::collections::HashSet<i64>,
}

/// The prepared `INSERT` statements of [`Parser::parse_solar_systems`].
struct SolarSystemStatements<'c> {
    system: rusqlite::Statement<'c>,
    subtype: rusqlite::Statement<'c>,
    disallowed_category: rusqlite::Statement<'c>,
    disallowed_group: rusqlite::Statement<'c>,
}

impl<'c> SolarSystemStatements<'c> {
    fn prepare(connection: &'c Connection) -> Result<Self, Error> {
        Ok(Self {
            system: connection.prepare(
            "INSERT INTO mapSolarSystems (solarSystemId, solarSystemName, constellationId, \
            type, luminosity, radius, centerX, centerY, centerZ, \
            security, securityClass, position2DX, position2DY, wormholeClassId, \
            factionId) \
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        )?,
            subtype: connection.prepare(
            "INSERT INTO mapSolarSystemSubType (solarSystemId, subType) VALUES (?1, ?2)",
        )?,
            disallowed_category: connection.prepare(
            "INSERT INTO mapSolarSystemDisallowedAnchorableCategories (solarSystemId, categoryId) \
            VALUES (?1, ?2)",
        )?,
            disallowed_group: connection.prepare(
            "INSERT INTO mapSolarSystemDisallowedAnchorableGroups (solarSystemId, groupId) \
            VALUES (?1, ?2)",
        )?,
        })
    }
}

impl Parser {
    /// Populates `mapSolarSystems` from
    /// `<sde_directory>/mapSolarSystems.jsonl`, filtering by
    /// `ParserConfig::system_in_scope` and accumulating the ids that pass the filter
    /// into `state.systems_in_scope`. Requires `mapConstellations` to
    /// already be populated (FK `mapSolarSystems.constellationId ->
    /// mapConstellations.constellationId`). Returns the number of rows
    /// inserted (out-of-scope systems do NOT count).
    ///
    /// The schema has no `projX`/`projY`/`projZ` columns: a system's 2D
    /// map position lives entirely in `position2DX`/`position2DY`.
    ///
    /// `position2DX`/`position2DY` follow
    /// [`ParserConfig::position_2d`]: CCP's precomputed value as-is
    /// ([`Position2DMode::Ccp`], `NULL` where the record carries none),
    /// or a locally-computed projection that ignores it --
    /// [`isometric_projection_2d`] or [`orthogonal_projection_2d`] per
    /// [`Position2DMode::Isometric`]/[`Position2DMode::Orthogonal`].
    ///
    /// `wormholeClassID` is read (it's needed for the scope filter above)
    /// and persisted as `wormholeClassId`.
    ///
    /// `hub`/`corridor`/`fringe` collapse into the single `type` column
    /// (confirmed mutually exclusive against real data). `border`/
    /// `regional`/`international` do NOT get the same treatment -- they
    /// genuinely overlap (104 real systems carry two or all three at
    /// once), so each one that's present gets its own row in
    /// `mapSolarSystemSubType` instead of collapsing into a column that
    /// would have to silently drop one of the values.
    ///
    /// `disallowedAnchorCategories`/`disallowedAnchorGroups` populate
    /// `mapSolarSystemDisallowedAnchorableCategories`/`...Groups` (one
    /// row per id present) -- confirmed independent of each other
    /// against real data (a system can restrict a specific group
    /// without its category appearing in its own
    /// `disallowedAnchorCategories`, and vice versa), and confirmed
    /// free of within-array duplicates across all 670 real records that
    /// carry either field, so inserting each id as encountered, without
    /// deduplicating first, doesn't risk a `PRIMARY KEY` violation.
    ///
    /// # Fields read from the source but not persisted as columns
    ///
    /// `regionID`, `starID`, `planetIDs`, and `stargateIDs` are all
    /// present in the real data (100%/95.3%/95.3%/62.0% of records
    /// respectively) but deliberately have no corresponding column:
    /// each is fully redundant with a relationship already captured
    /// from the *other* side. Confirmed against every real record
    /// checked (August 2026): `starID` always matches
    /// `mapStars.solarSystemID` (0 mismatches in 8089 checked),
    /// `planetIDs` always matches the set of `mapPlanets.solarSystemID`
    /// for that system (0 mismatches in 8088 checked), and `regionID`
    /// always matches the region derived by following
    /// `constellationID` through `mapRegions.constellationIDs` (0
    /// mismatches in all 8490 checked). `stargateIDs` isn't checked the
    /// same way here, but is the same kind of redundancy by design:
    /// the two-system connection it encodes is already derived from
    /// `mapSystemGates` by [`Self::parse_connections`], without needing
    /// this field at all. Storing any of the four as a column would
    /// just be a second copy of data already in the database, with no
    /// mechanism keeping the two in sync if they were ever expected to
    /// diverge.
    #[tracing::instrument(skip(state))]
    pub fn parse_solar_systems(
        &self,
        connection: &Connection,
        state: &mut SystemScopeState,
    ) -> Result<usize, Error> {
        let mut statements = SolarSystemStatements::prepare(connection)?;
        self.for_each_record("mapSolarSystems", "solar systems", |record| {
            self.insert_solar_system(&mut statements, state, record)
        })
    }

    /// Inserts one `mapSolarSystems` record and its junction rows; `false`
    /// if the system is out of scope.
    fn insert_solar_system(
        &self,
        statements: &mut SolarSystemStatements,
        state: &mut SystemScopeState,
        record: &Value,
    ) -> Result<bool, Error> {
        let system_id = self.required_i64(record, "_key")?;
        let wormhole_class_id = self.optional_i64(record, "wormholeClassID");
        if !self.config.system_in_scope(wormhole_class_id) {
            return Ok(false);
        }
        state.systems_in_scope.insert(system_id);

        let values = self.solar_system_values(record, system_id, wormhole_class_id)?;
        statements
            .system
            .execute(rusqlite::params_from_iter(values))?;

        // Unlike hub/corridor/fringe, border/regional/international
        // are NOT mutually exclusive (confirmed: 104 real systems
        // carry two or all three at once) -- each one that's true
        // gets its own row in mapSolarSystemSubType, instead of
        // collapsing into a single column the way `type` does.
        for subtype in ["border", "regional", "international"] {
            if self.optional_bool(record, subtype) == Some(true) {
                statements
                    .subtype
                    .execute(rusqlite::params![system_id, subtype])?;
            }
        }

        // disallowedAnchorCategories/disallowedAnchorGroups are
        // independent arrays (confirmed: neither can be derived
        // from the other via invGroups.categoryId), so each gets
        // its own junction table, populated the same way as
        // subType above -- one row per id present.
        for category_id in self.optional_i64_array(record, "disallowedAnchorCategories")? {
            statements
                .disallowed_category
                .execute(rusqlite::params![system_id, category_id])?;
        }
        for group_id in self.optional_i64_array(record, "disallowedAnchorGroups")? {
            statements
                .disallowed_group
                .execute(rusqlite::params![system_id, group_id])?;
        }
        Ok(true)
    }

    /// The `mapSolarSystems` columns of one record, in table order.
    fn solar_system_values(
        &self,
        record: &Value,
        system_id: i64,
        wormhole_class_id: Option<i64>,
    ) -> Result<Vec<SqlValue>, Error> {
        let name = self.config.required_localized(record, "name")?.to_owned();
        let constellation_id = self.required_i64(record, "constellationID")?;
        let luminosity = self.optional_f64(record, "luminosity");
        let radius = self.required_f64(record, "radius")?;
        let center @ (center_x, center_y, center_z) = self.required_position(record)?;
        let security = self.required_f64(record, "securityStatus")?;
        let security_class = self
            .optional_str(record, "securityClass")
            .map(str::to_owned);
        let (position_2d_x, position_2d_y) = self.system_position_2d(record, center);
        Ok(vec![
            system_id.into(),
            name.into(),
            constellation_id.into(),
            self.system_type(record).map(str::to_owned).into(),
            luminosity.into(),
            radius.into(),
            center_x.into(),
            center_y.into(),
            center_z.into(),
            security.into(),
            security_class.into(),
            position_2d_x.into(),
            position_2d_y.into(),
            wormhole_class_id.into(),
            self.optional_i64(record, "factionID").into(),
        ])
    }

    /// hub/corridor/fringe are confirmed mutually exclusive against
    /// real data (never two at once across 8490 real records) --
    /// collapsed into a single `type` column instead of three
    /// separate booleans. Order doesn't matter here precisely
    /// because they never co-occur.
    fn system_type(&self, record: &Value) -> Option<&'static str> {
        ["hub", "corridor", "fringe"]
            .into_iter()
            .find(|flag| self.optional_bool(record, flag) == Some(true))
    }

    /// The 2D map position of a system, according to `config.position_2d`.
    fn system_position_2d(
        &self,
        record: &Value,
        (x, y, z): (f64, f64, f64),
    ) -> (Option<f64>, Option<f64>) {
        match self.config.position_2d {
            Position2DMode::Ccp => (
                self.optional_nested_f64(record, "position2D", "x"),
                self.optional_nested_f64(record, "position2D", "y"),
            ),
            Position2DMode::Isometric(axis) => {
                let (x2d, y2d) = isometric_projection_2d(x, y, z, axis);
                (Some(x2d), Some(y2d))
            }
            Position2DMode::Orthogonal(axis) => {
                let (x2d, y2d) = orthogonal_projection_2d(x, y, z, axis);
                (Some(x2d), Some(y2d))
            }
        }
    }

    // mapSystemGates
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::parser::test_support::TempSdeDir;

    #[test]
    fn isometric_projection_2d_matches_known_reference_values() {
        // Known-correct reference values for x=100.0, y=200.0, z=300.0,
        // for each projected_axis (0/1/2), taking the two non-zero-forced
        // components from the 3-tuple.
        let (x, y, z) = (100.0, 200.0, 300.0);

        assert_eq!(
            isometric_projection_2d(x, y, z, ProjectedAxis::X),
            (100.0, 450.0)
        );
        assert_eq!(
            isometric_projection_2d(x, y, z, ProjectedAxis::Y),
            (-100.0, 450.0)
        );
        assert_eq!(
            isometric_projection_2d(x, y, z, ProjectedAxis::Z),
            (-200.0, 400.0)
        );
    }

    #[test]
    fn orthogonal_projection_2d_drops_the_given_axis() {
        // Known-correct reference values for x=100.0, y=200.0, z=300.0:
        // the dropped axis's component simply doesn't appear.
        let (x, y, z) = (100.0, 200.0, 300.0);

        assert_eq!(
            orthogonal_projection_2d(x, y, z, ProjectedAxis::X),
            (200.0, 300.0)
        );
        // The north-up top-down the map uses: (x, z).
        assert_eq!(
            orthogonal_projection_2d(x, y, z, ProjectedAxis::Y),
            (100.0, 300.0)
        );
        assert_eq!(
            orthogonal_projection_2d(x, y, z, ProjectedAxis::Z),
            (100.0, 200.0)
        );
    }

    #[test]
    fn system_in_scope_kspace_gates_on_map_kspace() {
        let mut config = ParserConfig::default();
        assert!(config.system_in_scope(None)); // default: map_kspace=true
        config.map_kspace = false;
        assert!(!config.system_in_scope(None));
    }

    #[test]
    fn system_in_scope_wormhole_gates_on_any_of_three_flags() {
        let mut config = ParserConfig {
            map_wspace: false,
            map_abyssal: false,
            map_void: false,
            ..Default::default()
        };
        assert!(!config.system_in_scope(Some(5)));
        config.map_wspace = true;
        assert!(config.system_in_scope(Some(5)));
    }

    #[test]
    #[allow(clippy::type_complexity)]
    fn parse_solar_systems_inserts_kspace_system_with_ccp_position2d() {
        let dir = TempSdeDir::new(
            "solar_systems_kspace",
            &[(
                "mapSolarSystems.jsonl",
                "{\"_key\": 30000142, \"name\": {\"en\": \"Jita\"}, \"constellationID\": 20000020, \
                 \"radius\": 999999999.0, \"position\": {\"x\": -100.0, \"y\": 200.0, \"z\": -300.0}, \
                 \"securityStatus\": 0.9459, \"securityClass\": \"B\", \"corridor\": false, \
                 \"fringe\": false, \"hub\": true, \"international\": true, \"regional\": true, \
                 \"factionID\": 500001, \"disallowedAnchorCategories\": [22, 65], \
                 \"luminosity\": 0.049, \"position2D\": {\"x\": 12.5, \"y\": -7.25}}\n",
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
                "INSERT INTO factions \
                 (factionId, factionName, iconId, sizeFactor, uniqueName, description) \
                 VALUES (500001, 'Caldari State', 1, 1.0, 1, 'x')",
                [],
            )
            .unwrap();
        connection
            .execute_batch(
                "INSERT INTO invCategories (categoryId, categoryName, published) VALUES \
                 (22, 'Deployable', 1), (65, 'Structure', 1);",
            )
            .unwrap();
        let config = ParserConfig::default();
        let parser = Parser::new(&dir.path, config);
        let mut scope = SystemScopeState::default();

        let count = parser.parse_solar_systems(&connection, &mut scope).unwrap();
        assert_eq!(count, 1);
        assert!(scope.systems_in_scope.contains(&30000142));

        let (
            name,
            security,
            security_class,
            p2dx,
            p2dy,
            wormhole_class_id,
            system_type,
            faction_id,
        ): (
            String,
            f64,
            String,
            f64,
            f64,
            Option<i64>,
            Option<String>,
            Option<i64>,
        ) = connection
            .query_row(
                "SELECT solarSystemName, security, securityClass, \
                     position2DX, position2DY, wormholeClassId, type, factionId \
                     FROM mapSolarSystems WHERE solarSystemId = 30000142",
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
        assert_eq!(name, "Jita");
        assert_eq!(security, 0.9459);
        assert_eq!(security_class, "B");
        // position2D unforced: the one the record already carries
        // (12.5, -7.25), NOT the one isometric_projection_2d would
        // compute ((-300, -250), see the forcing test further below).
        assert_eq!((p2dx, p2dy), (12.5, -7.25));
        // K-space systems have no wormholeClassID at all in real data.
        assert_eq!(wormhole_class_id, None);
        // hub: true in the fixture -> collapsed into type="hub".
        assert_eq!(system_type, Some("hub".to_string()));
        assert_eq!(faction_id, Some(500001));

        // disallowedAnchorCategories: [22, 65] in the fixture (real
        // values for Jita, August 2026) -> one row per id.
        let mut categories: Vec<i64> = connection
            .prepare(
                "SELECT categoryId FROM mapSolarSystemDisallowedAnchorableCategories \
                 WHERE solarSystemId = 30000142",
            )
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        categories.sort();
        assert_eq!(categories, vec![22, 65]);

        // disallowedAnchorGroups: absent from the fixture -> no rows at
        // all, not an error (confirms the absence case, complementing
        // the presence case above).
        let group_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM mapSolarSystemDisallowedAnchorableGroups \
                 WHERE solarSystemId = 30000142",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(group_count, 0);
    }

    #[test]
    fn parse_solar_systems_inserts_disallowed_anchor_groups() {
        let dir = TempSdeDir::new(
            "solar_systems_disallowed_groups",
            &[(
                "mapSolarSystems.jsonl",
                "{\"_key\": 30000001, \"name\": {\"en\": \"Sys\"}, \"constellationID\": 20000001, \
                 \"radius\": 1.0, \"position\": {\"x\": 0.0, \"y\": 0.0, \"z\": 0.0}, \
                 \"securityStatus\": 0.5, \"disallowedAnchorGroups\": [12, 340, 448]}\n",
            )],
        );
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        connection
            .execute(
                "INSERT INTO mapRegions \
                 (regionId, regionName, factionId, centerX, centerY, centerZ, nebula, wormholeClassId) \
                 VALUES (10000001, 'R', NULL, 0, 0, 0, 5, NULL)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO mapConstellations \
                 (constellationId, constellationName, regionId, centerX, centerY, centerZ) \
                 VALUES (20000001, 'C', 10000001, 0, 0, 0)",
                [],
            )
            .unwrap();
        // Real ids (August 2026): groupId 12/340/448 are all Container
        // variants (Cargo/Secure/Audit Log Secure), categoryId 2
        // (Celestial) -- unrelated to the categoryId 22/65 (Deployable/
        // Structure) used in the disallowedAnchorCategories test above,
        // confirming the two are independent in practice, not just in
        // the schema.
        connection
            .execute_batch(
                "INSERT INTO invCategories (categoryId, categoryName, published) VALUES \
                 (2, 'Celestial', 1); \
                 INSERT INTO invGroups (groupId, groupName, categoryId, anchorable) VALUES \
                 (12, 'Cargo Container', 2, 1), \
                 (340, 'Secure Cargo Container', 2, 1), \
                 (448, 'Audit Log Secure Container', 2, 1);",
            )
            .unwrap();

        let config = ParserConfig::default();
        let parser = Parser::new(&dir.path, config);
        let mut scope = SystemScopeState::default();
        let count = parser.parse_solar_systems(&connection, &mut scope).unwrap();
        assert_eq!(count, 1);

        let mut groups: Vec<i64> = connection
            .prepare(
                "SELECT groupId FROM mapSolarSystemDisallowedAnchorableGroups \
                 WHERE solarSystemId = 30000001",
            )
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        groups.sort();
        assert_eq!(groups, vec![12, 340, 448]);
    }

    #[test]
    fn parse_solar_systems_isometric_mode_ignores_ccp_position2d() {
        let dir = TempSdeDir::new(
            "solar_systems_force_isometric",
            &[(
                "mapSolarSystems.jsonl",
                "{\"_key\": 30000142, \"name\": {\"en\": \"Jita\"}, \"constellationID\": 20000020, \
                 \"radius\": 1.0, \"position\": {\"x\": -100.0, \"y\": 200.0, \"z\": -300.0}, \
                 \"securityStatus\": 0.9459, \"position2D\": {\"x\": 12.5, \"y\": -7.25}}\n",
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
        connection
            .execute(
                "INSERT INTO mapConstellations \
                 (constellationId, constellationName, regionId, centerX, centerY, centerZ) \
                 VALUES (20000020, 'Kimotoro', 10000002, 0, 0, 0)",
                [],
            )
            .unwrap();
        let config = ParserConfig {
            position_2d: Position2DMode::Isometric(ProjectedAxis::Y),
            ..Default::default()
        };
        let parser = Parser::new(&dir.path, config);
        let mut scope = SystemScopeState::default();

        parser.parse_solar_systems(&connection, &mut scope).unwrap();

        let (p2dx, p2dy): (f64, f64) = connection
            .query_row(
                "SELECT position2DX, position2DY FROM mapSolarSystems WHERE solarSystemId = 30000142",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        // Forced: should be the computed value (-300, -250), NOT the
        // (12.5, -7.25) the record carries.
        assert_eq!((p2dx, p2dy), (-300.0, -250.0));
    }

    #[test]
    fn parse_solar_systems_orthogonal_drops_y_for_north_up_top_down() {
        // Same fixture as the isometric force test, with the mode the
        // map actually uses: Orthogonal(Y) keeps (x, z) -- the
        // north-up top-down -- and still ignores CCP's position2D.
        let dir = TempSdeDir::new(
            "solar_systems_orthogonal",
            &[(
                "mapSolarSystems.jsonl",
                "{\"_key\": 30000142, \"name\": {\"en\": \"Jita\"}, \"constellationID\": 20000020, \
                 \"radius\": 1.0, \"position\": {\"x\": -100.0, \"y\": 200.0, \"z\": -300.0}, \
                 \"securityStatus\": 0.9459, \"position2D\": {\"x\": 12.5, \"y\": -7.25}}\n",
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
        connection
            .execute(
                "INSERT INTO mapConstellations \
                 (constellationId, constellationName, regionId, centerX, centerY, centerZ) \
                 VALUES (20000020, 'Kimotoro', 10000002, 0, 0, 0)",
                [],
            )
            .unwrap();
        let config = ParserConfig {
            position_2d: Position2DMode::Orthogonal(ProjectedAxis::Y),
            ..Default::default()
        };
        let parser = Parser::new(&dir.path, config);
        let mut scope = SystemScopeState::default();

        parser.parse_solar_systems(&connection, &mut scope).unwrap();

        let (p2dx, p2dy): (f64, f64) = connection
            .query_row(
                "SELECT position2DX, position2DY FROM mapSolarSystems WHERE solarSystemId = 30000142",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        // (x, z) = (-100, -300): y (the vertical axis) dropped, NOT the
        // (12.5, -7.25) the record carries.
        assert_eq!((p2dx, p2dy), (-100.0, -300.0));
    }

    #[test]
    fn parse_solar_systems_excludes_out_of_scope_systems() {
        let dir = TempSdeDir::new(
            "solar_systems_scope",
            &[(
                "mapSolarSystems.jsonl",
                "{\"_key\": 1, \"name\": {\"en\": \"KSpace\"}, \"constellationID\": 20000020, \
                 \"radius\": 1.0, \"position\": {\"x\": 0.0, \"y\": 0.0, \"z\": 0.0}, \
                 \"securityStatus\": 0.5}\n\
                 {\"_key\": 2, \"name\": {\"en\": \"WSpace\"}, \"constellationID\": 20000020, \
                 \"wormholeClassID\": 5, \"radius\": 1.0, \
                 \"position\": {\"x\": 0.0, \"y\": 0.0, \"z\": 0.0}, \"securityStatus\": -1.0}\n",
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
        connection
            .execute(
                "INSERT INTO mapConstellations \
                 (constellationId, constellationName, regionId, centerX, centerY, centerZ) \
                 VALUES (20000020, 'Kimotoro', 10000002, 0, 0, 0)",
                [],
            )
            .unwrap();
        // Excluir k-space; w-space sigue habilitado por default.
        let config = ParserConfig {
            map_kspace: false,
            ..Default::default()
        };
        let parser = Parser::new(&dir.path, config);
        let mut scope = SystemScopeState::default();

        let count = parser.parse_solar_systems(&connection, &mut scope).unwrap();
        assert_eq!(count, 1);
        assert!(!scope.systems_in_scope.contains(&1));
        assert!(scope.systems_in_scope.contains(&2));

        let total: i64 = connection
            .query_row("SELECT COUNT(*) FROM mapSolarSystems", [], |row| row.get(0))
            .unwrap();
        assert_eq!(total, 1);

        let wormhole_class_id: Option<i64> = connection
            .query_row(
                "SELECT wormholeClassId FROM mapSolarSystems WHERE solarSystemId = 2",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(wormhole_class_id, Some(5));
    }

    #[test]
    fn parse_solar_systems_missing_radius_errors() {
        let dir = TempSdeDir::new(
            "solar_systems_missing_radius",
            &[(
                "mapSolarSystems.jsonl",
                "{\"_key\": 1, \"name\": {\"en\": \"Test\"}, \"constellationID\": 20000020, \
                 \"position\": {\"x\": 0.0, \"y\": 0.0, \"z\": 0.0}, \"securityStatus\": 0.5}\n",
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
        connection
            .execute(
                "INSERT INTO mapConstellations \
                 (constellationId, constellationName, regionId, centerX, centerY, centerZ) \
                 VALUES (20000020, 'Kimotoro', 10000002, 0, 0, 0)",
                [],
            )
            .unwrap();
        let config = ParserConfig::default();
        let parser = Parser::new(&dir.path, config);
        let mut scope = SystemScopeState::default();

        let result = parser.parse_solar_systems(&connection, &mut scope);
        assert!(result.is_err());
    }

    #[test]
    fn parse_solar_systems_requires_a_security_status() {
        let dir = TempSdeDir::new(
            "solar_systems_missing_security",
            &[(
                "mapSolarSystems.jsonl",
                "{\"_key\": 1, \"name\": {\"en\": \"Test\"}, \"constellationID\": 20000020, \
                 \"radius\": 1.0, \"position\": {\"x\": 0.0, \"y\": 0.0, \"z\": 0.0}}\n",
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
        connection
            .execute(
                "INSERT INTO mapConstellations \
                 (constellationId, constellationName, regionId, centerX, centerY, centerZ) \
                 VALUES (20000020, 'Kimotoro', 10000002, 0, 0, 0)",
                [],
            )
            .unwrap();
        let parser = Parser::new(&dir.path, ParserConfig::default());
        let mut scope = SystemScopeState::default();

        let error = parser
            .parse_solar_systems(&connection, &mut scope)
            .unwrap_err();
        assert!(error.to_string().contains("securityStatus"), "{error}");

        let stored: i64 = connection
            .query_row("SELECT COUNT(*) FROM mapSolarSystems", [], |row| row.get(0))
            .unwrap();
        assert_eq!(stored, 0);
    }

    #[test]
    fn parse_solar_systems_security_must_stay_within_the_schema_range() {
        // One run of the parser per value, each over its own database.
        let attempt = |name: &str, security: f64| {
            let line = format!(
                "{{\"_key\": 1, \"name\": {{\"en\": \"Test\"}}, \"constellationID\": 20000020, \
                 \"radius\": 1.0, \"position\": {{\"x\": 0.0, \"y\": 0.0, \"z\": 0.0}}, \
                 \"securityStatus\": {security:?}}}\n"
            );
            let dir = TempSdeDir::new(name, &[("mapSolarSystems.jsonl", line.as_str())]);
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
            connection
                .execute(
                    "INSERT INTO mapConstellations \
                     (constellationId, constellationName, regionId, centerX, centerY, centerZ) \
                     VALUES (20000020, 'Kimotoro', 10000002, 0, 0, 0)",
                    [],
                )
                .unwrap();
            let parser = Parser::new(&dir.path, ParserConfig::default());
            let mut scope = SystemScopeState::default();
            parser
                .parse_solar_systems(&connection, &mut scope)
                .map(|_| ())
        };

        // The limits of the range are valid (-1.0 is the lowest a system gets).
        assert!(attempt("security_lowest", -1.0).is_ok());
        assert!(attempt("security_highest", 1.0).is_ok());
        // Past them the schema's CHECK refuses the row, and the parser fails.
        assert!(attempt("security_below", -1.0001).is_err());
        assert!(attempt("security_above", 1.0001).is_err());
    }
}
