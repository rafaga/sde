//! Populates the SDE data into the tables created by [`super::schema`].
//!
//! The state that needs sharing between [`Parser::parse_groups`] and
//! [`Parser::parse_types`] -- the "Sun" group's id and the set of
//! `typeId`s recognized as star types -- is passed explicitly via
//! [`StarTypeState`], rather than living on `self`.
//!
//! ## Contents
//!
//! Covers the "base" tables the rest of the entities reference via FK
//! and that don't depend on any map table: `invCategories`,
//! `invGroups`, `invTypes` (including the special star-type detection
//! that feeds `typeStar`), `races`, `npcCorporations` and `factions` +
//! `factionRace`.
//!
//! The map tables build up from there: `mapRegions` and
//! `mapConstellations` first (no isometric/dimetric projection
//! complexity), then `mapSolarSystems`, with the k-space/w-space/
//! abyssal/void scope filter (`ParserConfig::system_in_scope`) and the
//! isometric projection ([`isometric_projection_2d`]).
//! [`Parser::parse_stargates`] adds `mapSystemGates`, gated by
//! `config.with_gates` -- see its docstring for an important note on
//! why this particular function needs to run inside an explicit
//! transaction (it's not just an atomicity concern, unlike the rest of
//! the pipeline). [`Parser::parse_stars`] adds `mapStars` (its exact
//! shape confirmed against a real data sample -- see its docstring for
//! the detail); [`Parser::parse_planets`] adds `mapPlanets` (shape
//! confirmed against a real sample of 68407 records); [`Parser::parse_moons`]
//! adds `mapMoons`, gated by `config.with_moons` (see its docstring for
//! what remains an inference rather than a verification here).
//! [`Parser::parse_connections`] adds `mapSystemConnections` -- the
//! simplest of all: a single SQL statement that derives the connections
//! directly from `mapSystemGates`, without reading any SDE file at all.
//!
//! ## Supported file format
//!
//! JSONL only. Adding YAML support would mean pulling in a new
//! YAML-parsing dependency into the crate, a decision that was
//! deliberately left out of scope.
//!
//! ## Notable behavior
//!
//! - Every type in `types.jsonl` gets inserted unconditionally --
//!   nothing filters them out.
//! - If a "Sun"-group type's name doesn't have at least 3
//!   space-separated tokens, that type simply isn't treated as a star
//!   (it isn't inserted into `typeStar`) and the rest of the file keeps
//!   processing normally.
//! - Color extraction looks up the star's RGB code in the embedded
//!   `star_colors.json` by the first two characters of `parts[1]` (the
//!   spectral class, e.g. `"G5"`), not from the parenthesized token.
//!   An unknown spectral class is an `Error::data`, failing the whole
//!   parse.
//! - **Transactions**: this file's individual `parse_*` functions,
//!   called on their own, do NOT wrap their inserts in an explicit
//!   transaction (autocommit per INSERT, SQLite's default mode) -- only
//!   [`Parser::parse_data`], the orchestrator, wraps every phase in a
//!   single real transaction (`Connection::transaction()`), with "all
//!   or nothing" semantics: if any phase fails, EVERYTHING inserted up
//!   to that point gets rolled back. Calling an individual function
//!   directly, outside of `parse_data`, doesn't get that atomicity
//!   guarantee -- only `parse_data` provides it.
//! - [`Parser::parse_factions`] validates every element of
//!   `memberRaces` and returns `Error::data` on the first one
//!   that isn't an integer, since it would violate
//!   `factionRace.raceId INTEGER NOT NULL` on insert anyway -- failing
//!   early with a clear message beats a generic SQLite error further
//!   down.
//! - `mapRegions.nebula` is `INTEGER NOT NULL`, so
//!   [`Parser::parse_regions`] treats `nebulaID` as required (same
//!   criterion as `name`): it fails with `Error::data` and a
//!   clear message instead of letting SQLite reject it further down.
//! - [`Parser::parse_constellations`] computes the id as
//!   `constellationID` when present, falling back to `_key` otherwise
//!   (absent or present-but-not-an-integer) -- tolerant of either case,
//!   same result with well-formed data.
//! - `position2DX`/`position2DY` are how every 2D map projection is
//!   stored -- CCP's own precomputed value, or one computed locally
//!   via [`isometric_projection_2d`]/
//!   [`orthogonal_projection_2d`], depending on
//!   [`ParserConfig::position_2d`] (see [`Position2DMode`]).

use crate::Error;
use crate::builder::community::{self, CommunityConfig};
use crate::builder::usage::{self, FieldUsage};
use crate::objects::SdeFingerprint;
use reqwest::Client;
use rusqlite::Connection;
use rusqlite::types::Value as SqlValue;
use serde_json::Value;
use std::io::BufRead;
use std::path::Path;

/// Version of *what the parser reads* from the SDE. Bump it whenever a change
/// can make the parser read a field or table it didn't read before: a mirror
/// projected by an older version may lack it, so
/// [`super::mirror::Mirror::matches`] rejects it and the next update is a full
/// build. A new crate version that doesn't change what's read keeps it, and
/// keeps existing mirrors usable.
pub const PARSER_READS_VERSION: u32 = 1;

/// Version of *what the parser writes* for the same SDE. Bump it whenever a
/// change makes the same input produce a different `sde.db`.
/// [`Parser::build_database`] stores it as the database's
/// `PRAGMA user_version`, and [`super::update::prepare`] rebuilds a database
/// with another one from the mirror -- no download needed.
pub const PARSER_OUTPUT_VERSION: u32 = 0;

/// Config for the parser. Covers what's needed for localizing names
/// and how `position2DX`/`position2DY` are produced (see
/// [`Position2DMode`]/[`isometric_projection_2d`]/
/// [`orthogonal_projection_2d`]), plus the solar-system scope flags
/// (k-space/w-space/abyssal/void).
#[derive(Debug, Clone)]
pub struct ParserConfig {
    /// Language to extract from localized `name`/`description` fields
    /// (e.g. `{"en": "Jita", "es": "Jita"}` -> `"Jita"`), falling back
    /// to `"en"` if the requested language isn't there. Default `"en"`.
    pub language: String,
    /// How `position2DX`/`position2DY` are produced for every solar
    /// system: CCP's precomputed value as-is ([`Position2DMode::Ccp`]),
    /// or a locally-computed projection that **ignores** it
    /// ([`Position2DMode::Isometric`]/[`Position2DMode::Orthogonal`]).
    /// Default [`Position2DMode::Ccp`].
    pub position_2d: Position2DMode,
    /// Include k-space systems (no `wormholeClassID`). Default `true`.
    pub map_kspace: bool,
    /// Include wormhole space systems. Default `true`.
    pub map_wspace: bool,
    /// Include abyssal deadspace systems. Default `true`.
    pub map_abyssal: bool,
    /// Include "void" systems. Default `false`. See
    /// `ParserConfig::system_in_scope` for a note on why, today,
    /// `map_wspace`/`map_abyssal`/`map_void` all end up gating on the
    /// same check.
    pub map_void: bool,
    /// If `false`, [`Parser::parse_data`] skips the stargates phase
    /// ([`Parser::parse_stargates`]) entirely -- doesn't call it at all, not
    /// just filter its results. Default `true`.
    pub with_gates: bool,
    /// If `false`, [`Parser::parse_data`] skips the moons phase
    /// ([`Parser::parse_moons`]) entirely -- doesn't call it at all. Default
    /// `true`.
    pub with_moons: bool,
    /// if true, [`Parser::parse_data`] present elements bieng parsed in stdout as they are processed,
    /// otherwise it will be silent. Default `false`.
    pub verbose: bool,
    /// If `true`, [`Parser::build_database`] also fetches and layers in
    /// community-maintained data (`builder::community`) on top of the
    /// canonical SDE (ice belts, Jove Observatories, Triglavian
    /// invasion status, special ore anomalies, and
    /// `mapAbstractSystems` itself -- the one part of that layer that
    /// isn't gated by its own `CommunityConfig` flag). Default `false`:
    /// none of this comes from CCP's official export, so a database
    /// built with the default config contains canonical SDE data only.
    /// Has no effect on [`Parser::parse_data`] directly -- only
    /// [`Parser::build_database`], which calls both, consults it.
    pub with_third_party: bool,
}

impl Default for ParserConfig {
    fn default() -> Self {
        Self {
            language: "en".to_string(),
            position_2d: Position2DMode::default(),
            map_kspace: true,
            map_wspace: true,
            map_abyssal: true,
            map_void: false,
            with_gates: true,
            with_moons: true,
            verbose: false,
            with_third_party: false,
        }
    }
}

impl ParserConfig {
    /// Decides whether a solar system should be imported, based on the
    /// `map_kspace`/`map_wspace`/`map_abyssal`/`map_void` flags.
    ///
    /// The reworked SDE no longer splits k-space/w-space/abyssal/void by
    /// directory like the old one did; the only confirmed discriminator in
    /// the record itself is `wormholeClassID` (only present in systems that
    /// are NOT k-space). CCP doesn't expose a finer-grained flag to
    /// distinguish abyssal from void at this level, so
    /// `map_wspace`/`map_abyssal`/`map_void` currently share the
    /// same check ("does it have a `wormholeClassID`?").
    fn system_in_scope(&self, wormhole_class_id: Option<i64>) -> bool {
        match wormhole_class_id {
            None => self.map_kspace,
            Some(_) => self.map_wspace || self.map_abyssal || self.map_void,
        }
    }

    /// Reads a localized string field (`{"en": "...", "es": "...",
    /// ...}`), preferring `self.language`, falling back to `"en"`.
    /// Also accepts a plain (non-localized) string field. `None` if the
    /// field is absent or neither shape.
    fn localized<'a>(&self, record: &'a Value, field: &str) -> Option<&'a str> {
        match usage::field(record, field) {
            Some(Value::Object(map)) => map
                .get(self.language.as_str())
                .or_else(|| map.get("en"))
                .and_then(Value::as_str),
            Some(Value::String(s)) => Some(s.as_str()),
            _ => None,
        }
    }

    /// Same as [`Self::localized`], but a missing/unusable field is a
    /// data error (`Error::data`) instead of a silent `None`.
    fn required_localized<'a>(&self, record: &'a Value, field: &str) -> Result<&'a str, Error> {
        self.localized(record, field).ok_or_else(|| {
            Error::data(format!(
                "record has no localizable field `{field}` in `{}`/`en`: {record}",
                self.language
            ))
        })
    }
}

pub use crate::objects::Position2DMode;
/// Axis choice used by [`crate::objects::SdePoint::to_2d`] and
/// [`isometric_projection_2d`] -- moved to `crate::objects` (core, not
/// gated by the `builder` feature) since `SdePoint` needs it too, on
/// the read side. Re-exported here so existing `parser::ProjectedAxis`
/// references throughout this module keep working unchanged.
pub use crate::objects::ProjectedAxis;

/// Iterates the records in `<sde_directory>/<stem>.jsonl`, one
/// non-empty line at a time, as [`serde_json::Value`].
///
/// Each record carries its own `_key` field (the id) by convention of
/// the SDE.
fn iter_jsonl_records(
    sde_directory: &Path,
    stem: &str,
) -> Result<impl Iterator<Item = Result<Value, Error>>, Error> {
    let path = sde_directory.join(format!("{stem}.jsonl"));
    let file = std::fs::File::open(&path)?;
    let reader = std::io::BufReader::new(file);
    Ok(reader.lines().filter_map(|line| match line {
        Ok(line) if line.trim().is_empty() => None,
        Ok(line) => Some(serde_json::from_str::<Value>(&line).map_err(Error::from)),
        Err(err) => Some(Err(Error::from(err))),
    }))
}

// `Debug` (not derived before): needed so `#[tracing::instrument]` can
// capture `self` on `Parser`'s methods below -- both fields are cheap and
// meaningful to see on a span (which SDE directory, which config flags),
// unlike `SdeManager` in `lib.rs` (skipped there instead: it carries the
// whole in-memory universe).
#[derive(Debug)]
pub struct Parser {
    sde_directory: std::path::PathBuf,
    config: ParserConfig,
    /// What the last [`Self::parse_data`] read (see [`Self::field_usage`]).
    /// A `Mutex` rather than a `RefCell` so `Parser` stays `Sync`:
    /// [`Self::build_database`] holds `&self` across `.await`s.
    field_usage: std::sync::Mutex<Option<FieldUsage>>,
}

impl Parser {
    pub fn new(sde_directory: &Path, config: ParserConfig) -> Self {
        Self {
            sde_directory: sde_directory.to_path_buf(),
            config,
            field_usage: std::sync::Mutex::new(None),
        }
    }

    /// The config this parser was created with.
    pub fn config(&self) -> &ParserConfig {
        &self.config
    }

    /// The fields of each SDE table the last successful
    /// [`Self::parse_data`] (or [`Self::build_database`]) read, or `None`
    /// before one has run. See [`usage`].
    pub fn field_usage(&self) -> Option<FieldUsage> {
        self.field_usage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Runs `handle` on every record of `<sde_directory>/<stem>.jsonl` and
    /// returns how many it counted (`Ok(true)`); `Ok(false)` skips a record
    /// without counting it.
    ///
    /// Each record is the root of the field paths [`usage`] records while
    /// [`Self::parse_data`] runs.
    fn for_each_record(
        &self,
        stem: &str,
        label: &str,
        mut handle: impl FnMut(&Value) -> Result<bool, Error>,
    ) -> Result<usize, Error> {
        let mut count = 0usize;
        usage::open_table(stem);
        for record in iter_jsonl_records(&self.sde_directory, stem)? {
            let record = record?;
            usage::enter_record(stem, &record);
            let counted = handle(&record);
            usage::leave_record();
            if counted? {
                count += 1;
            }
        }
        if self.config.verbose {
            tracing::info!("Parsed {count} {label}");
        }
        Ok(count)
    }

    /// Extracts a required integer field from the record: if the field
    /// isn't present or isn't numeric, this is a data error
    /// (`Error::data`), not a silent `None`.
    fn required_i64(&self, record: &Value, field: &str) -> Result<i64, Error> {
        usage::field(record, field)
            .and_then(Value::as_i64)
            .ok_or_else(|| {
                Error::data(format!(
                    "record missing required field `{field}` (or it's not an integer): {record}"
                ))
            })
    }

    /// Extracts an optional integer field (`None` if missing, no error).
    fn optional_i64(&self, record: &Value, field: &str) -> Option<i64> {
        usage::field(record, field).and_then(Value::as_i64)
    }

    /// Extracts an optional boolean field.
    fn optional_bool(&self, record: &Value, field: &str) -> Option<bool> {
        usage::field(record, field).and_then(Value::as_bool)
    }

    /// Extracts an optional floating-point field.
    fn optional_f64(&self, record: &Value, field: &str) -> Option<f64> {
        usage::field(record, field).and_then(Value::as_f64)
    }

    /// Extracts a required plain string field (not localized -- for fields
    /// like `tickerName` that don't carry per-language variants).
    fn required_str<'a>(&self, record: &'a Value, field: &str) -> Result<&'a str, Error> {
        usage::field(record, field)
            .and_then(Value::as_str)
            .ok_or_else(|| {
                Error::data(format!(
                    "record missing required field `{field}` (or it's not a string): {record}"
                ))
            })
    }

    /// Extracts a required boolean field.
    fn required_bool(&self, record: &Value, field: &str) -> Result<bool, Error> {
        usage::field(record, field)
            .and_then(Value::as_bool)
            .ok_or_else(|| {
                Error::data(format!(
                    "record missing required field `{field}` (or it's not a boolean): {record}"
                ))
            })
    }

    /// Extracts a required floating-point field.
    fn required_f64(&self, record: &Value, field: &str) -> Result<f64, Error> {
        usage::field(record, field)
            .and_then(Value::as_f64)
            .ok_or_else(|| {
                Error::data(format!(
                    "record missing required field `{field}` (or it's not a number): {record}"
                ))
            })
    }

    /// Extracts ids from an optional integer array -- empty if the field is
    /// missing or `null`. If the field IS present but isn't an array, or
    /// any of its elements isn't an integer, that's a data error.
    fn optional_i64_array(&self, record: &Value, field: &str) -> Result<Vec<i64>, Error> {
        match usage::field(record, field) {
            None | Some(Value::Null) => Ok(Vec::new()),
            Some(Value::Array(items)) => items
                .iter()
                .map(|item| {
                    item.as_i64().ok_or_else(|| {
                        Error::data(format!("non-integer element in array `{field}`: {item}"))
                    })
                })
                .collect(),
            Some(other) => Err(Error::data(format!(
                "field `{field}` is not an array: {other}"
            ))),
        }
    }

    /// Extracts `record["position"]["x"/"y"/"z"]` as `(f64, f64, f64)`.
    /// Both levels are required; if `position` or any of its three
    /// components is missing, that's a data error.
    fn required_position(&self, record: &Value) -> Result<(f64, f64, f64), Error> {
        let position = usage::field(record, "position").ok_or_else(|| {
            Error::data(format!(
                "record missing required field `position`: {record}"
            ))
        })?;
        let x = self.required_f64(position, "x")?;
        let y = self.required_f64(position, "y")?;
        let z = self.required_f64(position, "z")?;
        Ok((x, y, z))
    }

    /// Extracts `record[outer][inner]` as a required `i64` -- used for
    /// `destination.stargateID`/`destination.solarSystemID` in
    /// [`Self::parse_stargates`].
    fn required_nested_i64(&self, record: &Value, outer: &str, inner: &str) -> Result<i64, Error> {
        let outer_val = usage::field(record, outer).ok_or_else(|| {
            Error::data(format!("record missing required field `{outer}`: {record}"))
        })?;
        self.required_i64(outer_val, inner)
    }

    /// Extracts an optional integer field that can either be at the
    /// record's top level or nested under `nested_field` (e.g.
    /// `statistics`), with the top level taking priority -- used for
    /// `radius`/`locked` in [`Self::parse_stars`]. A key present but
    /// `null` and a key that's entirely absent are treated alike here
    /// (both fall through to the nested value), since `optional_i64`
    /// doesn't distinguish "absent" from "present but of the wrong
    /// type/null".
    ///
    /// Both places are looked up every time, not only when the top level
    /// is missing, so [`usage`] records both: a later SDE that moves the
    /// field from one to the other must not be taken for an irrelevant
    /// change.
    fn optional_i64_with_nested_fallback(
        &self,
        record: &Value,
        field: &str,
        nested_field: &str,
    ) -> Option<i64> {
        let top = self.optional_i64(record, field);
        let nested =
            usage::field(record, nested_field).and_then(|nested| self.optional_i64(nested, field));
        top.or(nested)
    }

    /// Same as `optional_i64_with_nested_fallback`, but for boolean
    /// fields (e.g. `locked`).
    fn optional_bool_with_nested_fallback(
        &self,
        record: &Value,
        field: &str,
        nested_field: &str,
    ) -> Option<bool> {
        let top = self.optional_bool(record, field);
        let nested =
            usage::field(record, nested_field).and_then(|nested| self.optional_bool(nested, field));
        top.or(nested)
    }

    /// Same as `optional_i64_with_nested_fallback`, but for floating-point
    /// fields -- used for `mapPlanets.radius` (a `REAL` column, unlike
    /// `mapStars.radius`, which is `INTEGER`).
    fn optional_f64_with_nested_fallback(
        &self,
        record: &Value,
        field: &str,
        nested_field: &str,
    ) -> Option<f64> {
        let top = self.optional_f64(record, field);
        let nested =
            usage::field(record, nested_field).and_then(|nested| self.optional_f64(nested, field));
        top.or(nested)
    }

    /// Extracts an optional plain string field.
    fn optional_str<'a>(&self, record: &'a Value, field: &str) -> Option<&'a str> {
        usage::field(record, field).and_then(Value::as_str)
    }

    /// Extracts `record[outer][inner]` as `f64`, returning `None` if either
    /// level is missing (or isn't numeric) -- used for `position2D.x`/`.y`,
    /// which, unlike `position` (see `required_position`), is optional at
    /// both levels.
    fn optional_nested_f64(&self, record: &Value, outer: &str, inner: &str) -> Option<f64> {
        usage::field(usage::field(record, outer)?, inner).and_then(Value::as_f64)
    }

    /// Runs the full parsing pipeline over `sde_directory`, in dependency
    /// order.
    ///
    /// Unlike the individual `parse_*` functions -- which autocommit each
    /// `INSERT` separately, see "Notable behavior" in the module's docstring --
    /// this function DOES wrap the whole pipeline in a single explicit
    /// transaction (`Connection::transaction()`). If
    /// any phase fails, EVERYTHING inserted up to that point gets rolled
    /// back -- nothing is left half-persisted -- because rusqlite's
    /// `Transaction` rolls back automatically on `Drop` if `.commit()` was
    /// never called, and each call below's `?` operator triggers exactly
    /// that early `Drop` when it propagates the error.
    ///
    /// Requires `&mut Connection` (not `&Connection` like the individual
    /// functions) because `Connection::transaction()` requires it.
    ///
    /// ## Coverage
    ///
    /// Populates categories, groups, types (+ `typeStar`), races, NPC
    /// corporations, factions (+ `factionRace`), regions, constellations,
    /// solar systems, stargates (gated by `config.with_gates`), stars,
    /// planets, moons (gated by `config.with_moons`), connections,
    /// `stationServices`, `stationOperations` (+ its two junction tables),
    /// and `npcStations`. `npcStations` runs last and unconditionally (no
    /// config flag gates it, same as most tables besides gates/moons), but
    /// its `orbitMoonId` resolution depends on `parse_moons`/`parse_planets`
    /// having already populated `mapMoons`/`mapPlanets` -- if
    /// `config.with_moons` was `false`, every station that would otherwise
    /// resolve to a moon resolves to neither instead (both
    /// `orbitMoonId`/`orbitPlanetId` `NULL`), same as the one
    /// genuinely-neither station in the real data. See
    /// [`Self::parse_npc_stations`]'s docstring for more on this table.
    #[tracing::instrument]
    pub fn parse_data(&self, connection: &mut Connection) -> Result<ParseSummary, Error> {
        usage::start();
        let parsed = self.parse_all(connection);
        let field_usage = usage::finish();
        if parsed.is_ok() {
            *self
                .field_usage
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = field_usage;
        }
        parsed
    }

    /// [`Self::parse_data`] without the field-usage recording around it.
    fn parse_all(&self, connection: &mut Connection) -> Result<ParseSummary, Error> {
        let tx = connection.transaction()?;

        let mut summary = ParseSummary::default();
        let mut state = StarTypeState::default();
        self.parse_catalog(&tx, &mut summary, &mut state)?;
        self.parse_universe(&tx, &mut summary, &state)?;
        self.parse_stations(&tx, &mut summary)?;

        Self::resolve_foreign_key_violations(&tx)?;

        tx.commit()?;

        summary.star_types = state.star_type_ids.len();
        Ok(summary)
    }

    /// Item types, races, corporations and factions.
    fn parse_catalog(
        &self,
        tx: &rusqlite::Transaction,
        summary: &mut ParseSummary,
        state: &mut StarTypeState,
    ) -> Result<(), Error> {
        summary.categories = self.parse_categories(tx)?;
        summary.groups = self.parse_groups(tx, state)?;
        summary.types = self.parse_types(tx, state)?;
        summary.races = self.parse_races(tx)?;
        summary.npc_corporation_divisions = self.parse_npc_corporation_divisions(tx)?;
        summary.npc_corporations = self.parse_npc_corporations(tx)?;
        summary.factions = self.parse_factions(tx)?;
        Ok(())
    }

    /// Regions down to moons, and the connections between systems.
    fn parse_universe(
        &self,
        tx: &rusqlite::Transaction,
        summary: &mut ParseSummary,
        state: &StarTypeState,
    ) -> Result<(), Error> {
        summary.regions = self.parse_regions(tx)?;
        summary.constellations = self.parse_constellations(tx)?;
        let mut scope = SystemScopeState::default();
        summary.solar_systems = self.parse_solar_systems(tx, &mut scope)?;
        if self.config.with_gates {
            summary.stargates = self.parse_stargates(tx, &scope)?;
        }
        summary.stars = self.parse_stars(tx, &scope, state)?;
        summary.planets = self.parse_planets(tx, &scope)?;
        if self.config.with_moons {
            summary.moons = self.parse_moons(tx, &scope)?;
        }
        summary.connections = self.parse_connections(tx)?;
        Ok(())
    }

    /// Station services, operations and the NPC stations themselves.
    fn parse_stations(
        &self,
        tx: &rusqlite::Transaction,
        summary: &mut ParseSummary,
    ) -> Result<(), Error> {
        summary.station_services = self.parse_station_services(tx)?;
        summary.station_operations = self.parse_station_operations(tx)?;
        summary.station_operation_services = Self::count_rows(tx, "stationOperationServices")?;
        summary.station_operation_types = Self::count_rows(tx, "stationOperationTypes")?;
        summary.npc_stations = self.parse_npc_stations(tx)?;
        Ok(())
    }

    /// Diagnostic: PRAGMA foreign_key_check runs within this transaction,
    /// before COMMIT, so it can point at exactly which row/table/FK is
    /// unsatisfied -- instead of letting a bare `tx.commit()` fail with
    /// SQLite's generic "FOREIGN KEY constraint failed" (no indication of
    /// which of this crate's several DEFERRABLE constraints -- across
    /// npcCorporations/npcStations/factions -- is the actual culprit).
    /// Real EVE data is large enough (thousands of NPC corporations) that
    /// guessing at the cause from the generic message alone isn't
    /// reliable; this turns a silent COMMIT failure into a precise,
    /// actionable one. foreign_key_check only gives a numeric fk index
    /// (not a column name), so foreign_key_list(<table>) is queried too
    /// (cached per table, since multiple violations often share one) to
    /// translate that index into the actual column.
    ///
    /// One specific violation is known and expected, not a bug: real SDE
    /// data (confirmed against a real npcCorporations.jsonl/
    /// npcStations.jsonl sample, and again against a real user's full SDE
    /// build, August 2026) has exactly two corporations -- Doomheim
    /// (1000001, the sink corporation characters get moved to when
    /// deleted) and InterBus (1000148, an NPC courier service) -- whose
    /// `stationID` (60000001) matches no real station in npcStations.
    /// Neither corporation operates out of an actual station, so this
    /// isn't a parsing bug to fix; the FK is cleared to NULL for exactly
    /// this (table, column, parent) combination, right here, instead of
    /// failing the whole build over two corporations that were never
    /// going to resolve. No other DEFERRABLE column in this crate has any
    /// confirmed real instance of this -- every other violation still
    /// fails loudly below, since silently nulling out a column with no
    /// real-data evidence that it can legitimately be unresolved would
    /// risk masking an actual bug instead of a known data quirk.
    fn resolve_foreign_key_violations(tx: &rusqlite::Transaction) -> Result<(), Error> {
        let (violations, to_null) = Self::find_foreign_key_violations(tx)?;
        for rowid in to_null {
            tx.execute(
                "UPDATE npcCorporations SET stationId = NULL WHERE rowid = ?1",
                [rowid],
            )?;
        }
        if !violations.is_empty() {
            return Err(Error::data(format!(
                "foreign_key_check found {} unsatisfied constraint(s) before commit:\n  {}",
                violations.len(),
                violations.join("\n  ")
            )));
        }
        Ok(())
    }

    /// Runs `foreign_key_check` and sorts what it finds into the violations
    /// to report and the rowids of the known, expected ones to clear.
    fn find_foreign_key_violations(
        tx: &rusqlite::Transaction,
    ) -> Result<(Vec<String>, Vec<i64>), Error> {
        let mut fk_list_cache: std::collections::HashMap<
            String,
            std::collections::HashMap<i64, String>,
        > = std::collections::HashMap::new();
        let mut check = tx.prepare("PRAGMA foreign_key_check")?;
        let mut rows = check.query([])?;
        let mut violations = Vec::new();
        let mut to_null: Vec<i64> = Vec::new();
        while let Some(row) = rows.next()? {
            let table: String = row.get(0)?;
            let rowid: Option<i64> = row.get(1)?;
            let parent: String = row.get(2)?;
            let fkid: i64 = row.get(3)?;

            if !fk_list_cache.contains_key(&table) {
                fk_list_cache.insert(table.clone(), Self::foreign_key_columns(tx, &table)?);
            }
            let column = fk_list_cache
                .get(&table)
                .and_then(|m| m.get(&fkid))
                .map(String::as_str)
                .unwrap_or("<unknown column>");

            if Self::is_known_dangling_fk(&table, column, &parent)
                && let Some(rowid) = rowid
            {
                to_null.push(rowid);
                continue;
            }

            let rowid_str = rowid
                .map(|r| r.to_string())
                .unwrap_or_else(|| "N/A".to_string());
            violations.push(format!(
                "table {table}, rowid {rowid_str}, column {column} references {parent}"
            ));
        }
        Ok((violations, to_null))
    }

    /// The one known data quirk described above: an `npcCorporations.stationId`
    /// that matches no row of `npcStations`.
    fn is_known_dangling_fk(table: &str, column: &str, parent: &str) -> bool {
        table == "npcCorporations" && column == "stationId" && parent == "npcStations"
    }

    /// Maps each foreign key id of `table` (as reported by
    /// `foreign_key_check`) to the name of its column.
    fn foreign_key_columns(
        tx: &rusqlite::Transaction,
        table: &str,
    ) -> Result<std::collections::HashMap<i64, String>, Error> {
        let mut column_by_fkid = std::collections::HashMap::new();
        let mut fk_list = tx.prepare(&format!("PRAGMA foreign_key_list({table})"))?;
        let mut fk_rows = fk_list.query([])?;
        while let Some(fk_row) = fk_rows.next()? {
            let id: i64 = fk_row.get(0)?;
            let from_column: String = fk_row.get(3)?;
            column_by_fkid.insert(id, from_column);
        }
        Ok(column_by_fkid)
    }

    fn count_rows(tx: &rusqlite::Transaction, table: &str) -> Result<usize, Error> {
        let count: i64 = tx.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })?;
        Ok(count as usize)
    }

    /// Runs the full database build: the canonical SDE parse
    /// ([`Self::parse_data`]), plus -- gated behind `config.with_third_party`,
    /// off by default -- community-maintained data (`builder::community`) on
    /// top of it. Schema creation is the caller's responsibility, same as
    /// [`Self::parse_data`] already requires (both expect the schema to exist
    /// already).
    ///
    /// Exists so that a library consumer calling this crate directly (not
    /// through the `sde-builder` binary) gets the exact same
    /// canonical-vs-third-party behavior the CLI does, driven by the same
    /// [`ParserConfig`] -- centralizing that decision here instead of
    /// leaving it to every caller (CLI included) to reimplement the same
    /// `if config.with_third_party { community::process(...) }` check.
    ///
    /// `client`/`maps_url_base` are the same two pieces of information
    /// [`community::process`] itself needs -- passed through unchanged, not
    /// duplicated as separate config fields, so a caller that isn't using
    /// `with_third_party` doesn't need to supply a real `maps_url_base` at
    /// all (any string works; it's never read).
    ///
    /// `sde_build`, if given, is recorded verbatim in the
    /// `sdeFingerprint` table's `sdeBuild` column -- this function
    /// doesn't validate it or fetch it itself (that's `sde_index`'s
    /// job); pass `None` if the caller doesn't have one (e.g. a library
    /// consumer that isn't going through `sde_index::update_as_needed`
    /// at all). See [`objects::SdeFingerprint`] for what "fingerprint"
    /// means here and its real limits.
    #[tracing::instrument]
    pub async fn build_database(
        &self,
        connection: &mut Connection,
        client: &Client,
        maps_url_base: &str,
        sde_build: Option<&str>,
    ) -> Result<ParseSummary, Error> {
        let summary = self.parse_data(connection)?;

        let community_config = if self.config.with_third_party {
            let community_config = CommunityConfig {
                with_icebelts: true,
                with_triglavian_status: true,
                with_jove_observatories: true,
                with_special_ore: true,
            };
            community::process(
                connection,
                client,
                &self.sde_directory,
                maps_url_base,
                &community_config,
            )
            .await?;
            Some(community_config)
        } else {
            None
        };

        self.write_fingerprint(connection, sde_build, community_config.as_ref())?;
        connection.pragma_update(None, "user_version", PARSER_OUTPUT_VERSION)?;

        Ok(summary)
    }

    /// Writes the single row of `sdeFingerprint`, recording the exact
    /// settings this database was built with (see
    /// [`objects::SdeFingerprint`]). Called automatically by
    /// [`Self::build_database`] -- not `pub`, since there's no reason to
    /// call it on its own outside of that.
    fn write_fingerprint(
        &self,
        connection: &Connection,
        sde_build: Option<&str>,
        community_config: Option<&CommunityConfig>,
    ) -> Result<(), Error> {
        let fingerprint = SdeFingerprint {
            sde_build: sde_build.map(str::to_string),
            language: self.config.language.clone(),
            position_2d: self.config.position_2d,
            map_kspace: self.config.map_kspace,
            map_wspace: self.config.map_wspace,
            map_abyssal: self.config.map_abyssal,
            map_void: self.config.map_void,
            with_gates: self.config.with_gates,
            with_moons: self.config.with_moons,
            with_third_party: self.config.with_third_party,
            with_icebelts: community_config.map(|c| c.with_icebelts),
            with_triglavian_status: community_config.map(|c| c.with_triglavian_status),
            with_jove_observatories: community_config.map(|c| c.with_jove_observatories),
            with_special_ore: community_config.map(|c| c.with_special_ore),
        };
        let hash = fingerprint.hash();
        // `forceIsometricPosition2d`/`isometricProjectedAxis` encode
        // `Position2DMode` via `Position2DMode::fingerprint_columns`
        // (shared with `SdeManager::get_fingerprint`'s decode, so the
        // two can never disagree).
        let (force_column, axis_column) = fingerprint.position_2d.fingerprint_columns();
        connection.execute(
            "INSERT INTO sdeFingerprint (id, sdeBuild, language, \
            forceIsometricPosition2d, isometricProjectedAxis, mapKspace, mapWspace, \
            mapAbyssal, mapVoid, withGates, withMoons, withThirdParty, withIcebelts, \
            withTriglavianStatus, withJoveObservatories, withSpecialOre, hash) \
            VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            rusqlite::params![
                fingerprint.sde_build,
                fingerprint.language,
                force_column,
                axis_column,
                fingerprint.map_kspace,
                fingerprint.map_wspace,
                fingerprint.map_abyssal,
                fingerprint.map_void,
                fingerprint.with_gates,
                fingerprint.with_moons,
                fingerprint.with_third_party,
                fingerprint.with_icebelts,
                fingerprint.with_triglavian_status,
                fingerprint.with_jove_observatories,
                fingerprint.with_special_ore,
                hash,
            ],
        )?;
        Ok(())
    }
}

// ---------------------------------------------------------------------
// Orquestador
// ---------------------------------------------------------------------

/// Number of rows inserted by each phase of [`Parser::parse_data`].
///
/// `star_types` counts `typeStar`'s rows (not its own phase: they're
/// generated by [`Parser::parse_types`] when it detects "Sun"-group types).
/// `station_operation_services`/`station_operation_types` count rows
/// in those two junction tables (not their own phase either: they're
/// generated by [`Parser::parse_station_operations`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ParseSummary {
    pub categories: usize,
    pub groups: usize,
    pub types: usize,
    pub races: usize,
    pub npc_corporation_divisions: usize,
    pub npc_corporations: usize,
    pub factions: usize,
    pub star_types: usize,
    pub regions: usize,
    pub constellations: usize,
    pub solar_systems: usize,
    /// `0` both if there were no gates to import and if
    /// `config.with_gates` was `false` (in which case the phase doesn't
    /// even run) -- the two cases aren't distinguished.
    pub stargates: usize,
    pub stars: usize,
    pub planets: usize,
    /// `0` both if there were no moons to import and if
    /// `config.with_moons` was `false` -- the two cases aren't
    /// distinguished, same criterion as `stargates`.
    pub moons: usize,
    pub connections: usize,
    pub station_services: usize,
    pub station_operations: usize,
    pub station_operation_services: usize,
    pub station_operation_types: usize,
    pub npc_stations: usize,
}

mod connections;
pub use solar_systems::{SystemScopeState, isometric_projection_2d, orthogonal_projection_2d};
pub use types::StarTypeState;

mod constellations;
mod corporations;
mod factions;
mod moons;
mod planets;
mod races;
mod regions;
mod solar_systems;
mod stargates;
mod stars;
mod stations;
#[cfg(test)]
mod test_support;
mod types;

#[cfg(test)]
mod tests {
    use super::*;
    use test_support::TempSdeDir;

    #[test]
    fn localized_falls_back_to_english() {
        let config = ParserConfig {
            language: "fr".to_string(),
            ..Default::default()
        };
        let record: Value =
            serde_json::from_str(r#"{"name": {"en": "Jita", "de": "Jita"}}"#).unwrap();
        // "fr" isn't present -> falls back to "en".
        assert_eq!(config.localized(&record, "name"), Some("Jita"));
    }

    #[test]
    fn localized_uses_requested_language_when_present() {
        let config = ParserConfig {
            language: "de".to_string(),
            ..Default::default()
        };
        let record: Value =
            serde_json::from_str(r#"{"name": {"en": "Jita", "de": "Jita (de)"}}"#).unwrap();
        assert_eq!(config.localized(&record, "name"), Some("Jita (de)"));
    }

    #[test]
    fn parser_config_default_trusts_ccp_position_2d() {
        // Real defaults: trust the position2D CCP already provides
        // when it's present (`Position2DMode::Ccp`), no local
        // projection.
        let config = ParserConfig::default();
        assert_eq!(config.position_2d, Position2DMode::Ccp);
    }

    #[test]
    fn parse_data_happy_path_returns_summary_and_commits() {
        let dir = TempSdeDir::new(
            "parse_data_happy",
            &[
                (
                    "categories.jsonl",
                    "{\"_key\": 6, \"name\": {\"en\": \"Celestial\"}, \"published\": true}\n",
                ),
                (
                    "groups.jsonl",
                    "{\"_key\": 6, \"categoryID\": 6, \"name\": {\"en\": \"Sun\"}, \"anchorable\": false}\n\
                     {\"_key\": 7, \"categoryID\": 6, \"name\": {\"en\": \"Frigate\"}, \"anchorable\": false}\n",
                ),
                (
                    "races.jsonl",
                    "{\"_key\": 1, \"name\": {\"en\": \"Caldari\"}}\n",
                ),
                (
                    "npcCorporations.jsonl",
                    "{\"_key\": 1000004, \"name\": {\"en\": \"CBD Corporation\"}, \
                     \"tickerName\": \"CBD\", \"deleted\": false, \"extent\": \"L\", \
                     \"hasPlayerPersonnelManager\": false, \"initialPrice\": 0, \"memberLimit\": -1, \
                     \"minSecurity\": 0.0, \"minimumJoinStanding\": 1, \
                     \"sendCharTerminationMessage\": true, \"shares\": 1000, \"size\": \"L\", \
                     \"taxRate\": 0.0, \"uniqueName\": true, \"iconID\": 500, \"raceID\": 1}\n",
                ),
                (
                    "factions.jsonl",
                    "{\"_key\": 500001, \"name\": {\"en\": \"Caldari State\"}, \"iconID\": 600, \
                     \"sizeFactor\": 3.0, \"uniqueName\": true, \"description\": {\"en\": \"x\"}, \
                     \"corporationID\": 1000004, \"memberRaces\": [1]}\n",
                ),
                ("npcCorporationDivisions.jsonl", ""),
                ("stationServices.jsonl", ""),
                ("stationOperations.jsonl", ""),
                ("npcStations.jsonl", ""),
                (
                    "mapRegions.jsonl",
                    "{\"_key\": 10000002, \"name\": {\"en\": \"The Forge\"}, \"nebulaID\": 5, \
                     \"position\": {\"x\": 100.0, \"y\": 200.0, \"z\": 300.0}}\n",
                ),
                (
                    "mapConstellations.jsonl",
                    "{\"_key\": 20000020, \"name\": {\"en\": \"Kimotoro\"}, \"regionID\": 10000002, \
                     \"position\": {\"x\": 110.0, \"y\": 210.0, \"z\": 310.0}}\n",
                ),
                (
                    "mapSolarSystems.jsonl",
                    "{\"_key\": 30000142, \"name\": {\"en\": \"Jita\"}, \"constellationID\": 20000020, \
                     \"radius\": 999999999.0, \"position\": {\"x\": -100.0, \"y\": 200.0, \"z\": -300.0}, \
                     \"securityStatus\": 0.9459, \"securityClass\": \"B\", \"corridor\": false, \
                     \"fringe\": false, \"hub\": true, \"international\": true, \"regional\": true, \
                     \"luminosity\": 0.049, \"position2D\": {\"x\": 12.5, \"y\": -7.25}}\n\
                     {\"_key\": 30002187, \"name\": {\"en\": \"Perimeter\"}, \"constellationID\": 20000020, \
                     \"radius\": 1.0, \"position\": {\"x\": 0.0, \"y\": 0.0, \"z\": 0.0}, \
                     \"securityStatus\": 0.9}\n",
                ),
                (
                    "mapStargates.jsonl",
                    "{\"_key\": 50000001, \"solarSystemID\": 30000142, \"typeID\": 16, \
                     \"position\": {\"x\": 1.0, \"y\": 2.0, \"z\": 3.0}, \
                     \"destination\": {\"stargateID\": 50000002, \"solarSystemID\": 30002187}}\n\
                     {\"_key\": 50000002, \"solarSystemID\": 30002187, \"typeID\": 16, \
                     \"position\": {\"x\": 4.0, \"y\": 5.0, \"z\": 6.0}, \
                     \"destination\": {\"stargateID\": 50000001, \"solarSystemID\": 30000142}}\n",
                ),
                (
                    "mapStars.jsonl",
                    "{\"_key\": 40000001, \"radius\": 63350000, \"solarSystemID\": 30000142, \
                     \"statistics\": {\"age\": 4.5e17, \"life\": 6.9e17, \"luminosity\": 0.01575, \
                     \"spectralClass\": \"K2 V\", \"temperature\": 4567.0}, \"typeID\": 3000}\n",
                ),
                (
                    "mapPlanets.jsonl",
                    "{\"_key\": 40000002, \"celestialIndex\": 1, \
                     \"position\": {\"x\": 161891117336.0, \"y\": 21288951986.0, \"z\": -73529712226.0}, \
                     \"radius\": 5060000, \"solarSystemID\": 30000142, \
                     \"statistics\": {\"locked\": false}, \"typeID\": 11}\n",
                ),
                (
                    "mapMoons.jsonl",
                    "{\"_key\": 40000004, \"solarSystemID\": 30000142, \"orbitIndex\": 1, \
                     \"orbitID\": 40000002, \"typeID\": 12, \"radius\": 100000, \
                     \"position\": {\"x\": 1.0, \"y\": 2.0, \"z\": 3.0}}\n",
                ),
                (
                    "types.jsonl",
                    "{\"_key\": 3000, \"groupID\": 6, \"name\": {\"en\": \"Yellow G5 (ffcc00)\"}, \
                     \"iconID\": 100, \"published\": true, \"volume\": 0.0}\n\
                     {\"_key\": 16, \"groupID\": 7, \"name\": {\"en\": \"Stargate\"}, \"published\": true}\n\
                     {\"_key\": 11, \"groupID\": 7, \"name\": {\"en\": \"Planet (Barren)\"}, \"published\": true}\n\
                     {\"_key\": 12, \"groupID\": 7, \"name\": {\"en\": \"Moon\"}, \"published\": true}\n",
                ),
            ],
        );
        let mut connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        let config = ParserConfig::default();
        let parser = Parser::new(&dir.path, config);

        let summary = parser.parse_data(&mut connection).unwrap();
        assert_eq!(
            summary,
            ParseSummary {
                categories: 1,
                groups: 2,
                types: 4,
                races: 1,
                npc_corporation_divisions: 0,
                npc_corporations: 1,
                factions: 1,
                star_types: 1,
                regions: 1,
                constellations: 1,
                solar_systems: 2,
                stargates: 2,
                stars: 1,
                planets: 1,
                moons: 1,
                connections: 1,
                station_services: 0,
                station_operations: 0,
                station_operation_services: 0,
                station_operation_types: 0,
                npc_stations: 0,
            }
        );

        let total_faction_race: i64 = connection
            .query_row("SELECT COUNT(*) FROM factionRace", [], |row| row.get(0))
            .unwrap();
        assert_eq!(total_faction_race, 1);

        let (conn_system_a, conn_system_b): (i64, i64) = connection
            .query_row(
                "SELECT systemA, systemB FROM mapSystemConnections",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!((conn_system_a, conn_system_b), (30000142, 30002187));

        let (dest_gate, dest_system): (i64, i64) = connection
            .query_row(
                "SELECT destinationGateId, destinationSystemId FROM mapSystemGates \
                 WHERE systemGateId = 50000001",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(dest_gate, 50000002);
        assert_eq!(dest_system, 30002187);
    }

    #[tokio::test]
    async fn build_database_writes_fingerprint_row() {
        let dir = TempSdeDir::new(
            "build_database_fingerprint",
            &[
                (
                    "categories.jsonl",
                    "{\"_key\": 6, \"name\": {\"en\": \"Celestial\"}, \"published\": true}\n",
                ),
                (
                    "groups.jsonl",
                    "{\"_key\": 6, \"categoryID\": 6, \"name\": {\"en\": \"Sun\"}, \"anchorable\": false}\n\
                     {\"_key\": 7, \"categoryID\": 6, \"name\": {\"en\": \"Frigate\"}, \"anchorable\": false}\n",
                ),
                (
                    "races.jsonl",
                    "{\"_key\": 1, \"name\": {\"en\": \"Caldari\"}}\n",
                ),
                (
                    "npcCorporations.jsonl",
                    "{\"_key\": 1000004, \"name\": {\"en\": \"CBD Corporation\"}, \
                     \"tickerName\": \"CBD\", \"deleted\": false, \"extent\": \"L\", \
                     \"hasPlayerPersonnelManager\": false, \"initialPrice\": 0, \"memberLimit\": -1, \
                     \"minSecurity\": 0.0, \"minimumJoinStanding\": 1, \
                     \"sendCharTerminationMessage\": true, \"shares\": 1000, \"size\": \"L\", \
                     \"taxRate\": 0.0, \"uniqueName\": true, \"iconID\": 500, \"raceID\": 1}\n",
                ),
                (
                    "factions.jsonl",
                    "{\"_key\": 500001, \"name\": {\"en\": \"Caldari State\"}, \"iconID\": 600, \
                     \"sizeFactor\": 3.0, \"uniqueName\": true, \"description\": {\"en\": \"x\"}, \
                     \"corporationID\": 1000004, \"memberRaces\": [1]}\n",
                ),
                ("npcCorporationDivisions.jsonl", ""),
                ("stationServices.jsonl", ""),
                ("stationOperations.jsonl", ""),
                ("npcStations.jsonl", ""),
                (
                    "mapRegions.jsonl",
                    "{\"_key\": 10000002, \"name\": {\"en\": \"The Forge\"}, \"nebulaID\": 5, \
                     \"position\": {\"x\": 100.0, \"y\": 200.0, \"z\": 300.0}}\n",
                ),
                (
                    "mapConstellations.jsonl",
                    "{\"_key\": 20000020, \"name\": {\"en\": \"Kimotoro\"}, \"regionID\": 10000002, \
                     \"position\": {\"x\": 110.0, \"y\": 210.0, \"z\": 310.0}}\n",
                ),
                (
                    "mapSolarSystems.jsonl",
                    "{\"_key\": 30000142, \"name\": {\"en\": \"Jita\"}, \"constellationID\": 20000020, \
                     \"radius\": 999999999.0, \"position\": {\"x\": -100.0, \"y\": 200.0, \"z\": -300.0}, \
                     \"securityStatus\": 0.9459, \"securityClass\": \"B\", \"corridor\": false, \
                     \"fringe\": false, \"hub\": true, \"international\": true, \"regional\": true, \
                     \"luminosity\": 0.049, \"position2D\": {\"x\": 12.5, \"y\": -7.25}}\n\
                     {\"_key\": 30002187, \"name\": {\"en\": \"Perimeter\"}, \"constellationID\": 20000020, \
                     \"radius\": 1.0, \"position\": {\"x\": 0.0, \"y\": 0.0, \"z\": 0.0}, \
                     \"securityStatus\": 0.9}\n",
                ),
                (
                    "mapStargates.jsonl",
                    "{\"_key\": 50000001, \"solarSystemID\": 30000142, \"typeID\": 16, \
                     \"position\": {\"x\": 1.0, \"y\": 2.0, \"z\": 3.0}, \
                     \"destination\": {\"stargateID\": 50000002, \"solarSystemID\": 30002187}}\n\
                     {\"_key\": 50000002, \"solarSystemID\": 30002187, \"typeID\": 16, \
                     \"position\": {\"x\": 4.0, \"y\": 5.0, \"z\": 6.0}, \
                     \"destination\": {\"stargateID\": 50000001, \"solarSystemID\": 30000142}}\n",
                ),
                (
                    "mapStars.jsonl",
                    "{\"_key\": 40000001, \"radius\": 63350000, \"solarSystemID\": 30000142, \
                     \"statistics\": {\"age\": 4.5e17, \"life\": 6.9e17, \"luminosity\": 0.01575, \
                     \"spectralClass\": \"K2 V\", \"temperature\": 4567.0}, \"typeID\": 3000}\n",
                ),
                (
                    "mapPlanets.jsonl",
                    "{\"_key\": 40000002, \"celestialIndex\": 1, \
                     \"position\": {\"x\": 161891117336.0, \"y\": 21288951986.0, \"z\": -73529712226.0}, \
                     \"radius\": 5060000, \"solarSystemID\": 30000142, \
                     \"statistics\": {\"locked\": false}, \"typeID\": 11}\n",
                ),
                (
                    "mapMoons.jsonl",
                    "{\"_key\": 40000004, \"solarSystemID\": 30000142, \"orbitIndex\": 1, \
                     \"orbitID\": 40000002, \"typeID\": 12, \"radius\": 100000, \
                     \"position\": {\"x\": 1.0, \"y\": 2.0, \"z\": 3.0}}\n",
                ),
                (
                    "types.jsonl",
                    "{\"_key\": 3000, \"groupID\": 6, \"name\": {\"en\": \"Yellow G5 (ffcc00)\"}, \
                     \"iconID\": 100, \"published\": true, \"volume\": 0.0}\n\
                     {\"_key\": 16, \"groupID\": 7, \"name\": {\"en\": \"Stargate\"}, \"published\": true}\n\
                     {\"_key\": 11, \"groupID\": 7, \"name\": {\"en\": \"Planet (Barren)\"}, \"published\": true}\n\
                     {\"_key\": 12, \"groupID\": 7, \"name\": {\"en\": \"Moon\"}, \"published\": true}\n",
                ),
            ],
        );

        let mut connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        let config = ParserConfig {
            language: "en".to_string(),
            position_2d: Position2DMode::Isometric(ProjectedAxis::Z),
            map_kspace: true,
            map_wspace: false,
            map_abyssal: false,
            map_void: false,
            with_gates: true,
            with_moons: true,
            with_third_party: false,
            ..Default::default()
        };
        let parser = Parser::new(&dir.path, config);
        let client = reqwest::Client::new();

        parser
            .build_database(
                &mut connection,
                &client,
                "http://example.invalid/",
                Some("3458726"),
            )
            .await
            .unwrap();

        use rusqlite::types::Value as Sql;
        let read = |column: &str| -> Sql {
            connection
                .query_row(
                    &format!("SELECT {column} FROM sdeFingerprint WHERE id = 1"),
                    [],
                    |row| row.get(0),
                )
                .unwrap()
        };
        let text = |value: &str| Sql::Text(value.to_string());

        assert_eq!(read("sdeBuild"), text("3458726"));
        assert_eq!(read("language"), text("en"));
        assert_eq!(read("forceIsometricPosition2d"), Sql::Integer(1));
        assert_eq!(read("isometricProjectedAxis"), text("Z"));
        assert_eq!(read("mapKspace"), Sql::Integer(1));
        assert_eq!(read("mapWspace"), Sql::Integer(0));
        assert_eq!(read("mapAbyssal"), Sql::Integer(0));
        assert_eq!(read("mapVoid"), Sql::Integer(0));
        assert_eq!(read("withGates"), Sql::Integer(1));
        assert_eq!(read("withMoons"), Sql::Integer(1));
        // with_third_party was false, so the four CommunityConfig flags
        // were never consulted -- confirms they're recorded as NULL,
        // not e.g. silently defaulted to false.
        assert_eq!(read("withThirdParty"), Sql::Integer(0));
        assert_eq!(read("withIcebelts"), Sql::Null);
        assert_eq!(read("withTriglavianStatus"), Sql::Null);
        assert_eq!(read("withJoveObservatories"), Sql::Null);
        assert_eq!(read("withSpecialOre"), Sql::Null);
        let Sql::Text(hash) = read("hash") else {
            panic!("the stored hash is not text");
        };
        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));

        // A second build_database() call would violate sdeFingerprint's
        // singleton PRIMARY KEY -- out of scope for this test (it would
        // need a second full parse_data() run first), but confirms the
        // schema-level guarantee is what protects against more than one
        // row existing, not application logic that could be bypassed.
    }

    #[test]
    fn parse_data_reports_precise_diagnostic_for_unsatisfied_deferred_fk() {
        // Same fixture as parse_data_happy_path_returns_summary_and_commits,
        // except npcCorporations carries an enemyID that never resolves
        // to any real corporation anywhere in the file -- confirms the
        // PRAGMA foreign_key_check diagnostic (run right before COMMIT)
        // correctly names the table and column, instead of just letting
        // the raw COMMIT fail with SQLite's generic, unspecific message.
        let dir = TempSdeDir::new(
            "parse_data_dangling_fk",
            &[
                (
                    "categories.jsonl",
                    "{\"_key\": 6, \"name\": {\"en\": \"Celestial\"}, \"published\": true}\n",
                ),
                (
                    "groups.jsonl",
                    "{\"_key\": 6, \"categoryID\": 6, \"name\": {\"en\": \"Sun\"}, \"anchorable\": false}\n\
                     {\"_key\": 7, \"categoryID\": 6, \"name\": {\"en\": \"Frigate\"}, \"anchorable\": false}\n",
                ),
                (
                    "races.jsonl",
                    "{\"_key\": 1, \"name\": {\"en\": \"Caldari\"}}\n",
                ),
                (
                    "npcCorporations.jsonl",
                    "{\"_key\": 1000004, \"name\": {\"en\": \"CBD Corporation\"}, \
                     \"tickerName\": \"CBD\", \"deleted\": false, \"extent\": \"L\", \
                     \"hasPlayerPersonnelManager\": false, \"initialPrice\": 0, \"memberLimit\": -1, \
                     \"minSecurity\": 0.0, \"minimumJoinStanding\": 1, \
                     \"sendCharTerminationMessage\": true, \"shares\": 1000, \"size\": \"L\", \
                     \"taxRate\": 0.0, \"uniqueName\": true, \"iconID\": 500, \"raceID\": 1, \
                     \"enemyID\": 999999999}\n",
                ),
                (
                    "factions.jsonl",
                    "{\"_key\": 500001, \"name\": {\"en\": \"Caldari State\"}, \"iconID\": 600, \
                     \"sizeFactor\": 3.0, \"uniqueName\": true, \"description\": {\"en\": \"x\"}, \
                     \"corporationID\": 1000004, \"memberRaces\": [1]}\n",
                ),
                ("npcCorporationDivisions.jsonl", ""),
                ("stationServices.jsonl", ""),
                ("stationOperations.jsonl", ""),
                ("npcStations.jsonl", ""),
                (
                    "mapRegions.jsonl",
                    "{\"_key\": 10000002, \"name\": {\"en\": \"The Forge\"}, \"nebulaID\": 5, \
                     \"position\": {\"x\": 100.0, \"y\": 200.0, \"z\": 300.0}}\n",
                ),
                (
                    "mapConstellations.jsonl",
                    "{\"_key\": 20000020, \"name\": {\"en\": \"Kimotoro\"}, \"regionID\": 10000002, \
                     \"position\": {\"x\": 110.0, \"y\": 210.0, \"z\": 310.0}}\n",
                ),
                (
                    "mapSolarSystems.jsonl",
                    "{\"_key\": 30000142, \"name\": {\"en\": \"Jita\"}, \"constellationID\": 20000020, \
                     \"radius\": 999999999.0, \"position\": {\"x\": -100.0, \"y\": 200.0, \"z\": -300.0}, \
                     \"securityStatus\": 0.9459}\n",
                ),
                ("mapStargates.jsonl", ""),
                ("mapStars.jsonl", ""),
                ("mapPlanets.jsonl", ""),
                ("mapMoons.jsonl", ""),
                ("types.jsonl", ""),
            ],
        );
        let mut connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        let config = ParserConfig::default();
        let parser = Parser::new(&dir.path, config);

        let error = parser.parse_data(&mut connection).unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains("npcCorporations"),
            "diagnostic should name the table: {message}"
        );
        assert!(
            message.contains("enemyId"),
            "diagnostic should name the actual column, not just a numeric fk index: {message}"
        );

        // Confirms the transaction genuinely rolled back -- the row with
        // the dangling enemyId never persisted.
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM npcCorporations", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn parse_data_clears_dangling_npc_corporation_station_id_instead_of_failing() {
        // Same fixture as the previous test, except the dangling reference
        // is specifically npcCorporations.stationId -> npcStations (not
        // enemyId) -- the one confirmed-real case (Doomheim/InterBus, both
        // stationID 60000001, neither a real station) that parse_data()
        // resolves automatically instead of failing the whole build.
        let dir = TempSdeDir::new(
            "parse_data_dangling_station_id",
            &[
                (
                    "categories.jsonl",
                    "{\"_key\": 6, \"name\": {\"en\": \"Celestial\"}, \"published\": true}\n",
                ),
                (
                    "groups.jsonl",
                    "{\"_key\": 6, \"categoryID\": 6, \"name\": {\"en\": \"Sun\"}, \"anchorable\": false}\n\
                     {\"_key\": 7, \"categoryID\": 6, \"name\": {\"en\": \"Frigate\"}, \"anchorable\": false}\n",
                ),
                (
                    "races.jsonl",
                    "{\"_key\": 1, \"name\": {\"en\": \"Caldari\"}}\n",
                ),
                (
                    "npcCorporations.jsonl",
                    "{\"_key\": 1000001, \"name\": {\"en\": \"Doomheim\"}, \
                     \"tickerName\": \"D\", \"deleted\": false, \"extent\": \"L\", \
                     \"hasPlayerPersonnelManager\": false, \"initialPrice\": 0, \"memberLimit\": -1, \
                     \"minSecurity\": 0.0, \"minimumJoinStanding\": 1, \
                     \"sendCharTerminationMessage\": true, \"shares\": 1000, \"size\": \"L\", \
                     \"taxRate\": 0.0, \"uniqueName\": true, \"iconID\": 500, \"raceID\": 1, \
                     \"stationID\": 60000001}\n",
                ),
                ("factions.jsonl", ""),
                ("npcCorporationDivisions.jsonl", ""),
                ("stationServices.jsonl", ""),
                ("stationOperations.jsonl", ""),
                ("npcStations.jsonl", ""),
                (
                    "mapRegions.jsonl",
                    "{\"_key\": 10000002, \"name\": {\"en\": \"The Forge\"}, \"nebulaID\": 5, \
                     \"position\": {\"x\": 100.0, \"y\": 200.0, \"z\": 300.0}}\n",
                ),
                (
                    "mapConstellations.jsonl",
                    "{\"_key\": 20000020, \"name\": {\"en\": \"Kimotoro\"}, \"regionID\": 10000002, \
                     \"position\": {\"x\": 110.0, \"y\": 210.0, \"z\": 310.0}}\n",
                ),
                (
                    "mapSolarSystems.jsonl",
                    "{\"_key\": 30000142, \"name\": {\"en\": \"Jita\"}, \"constellationID\": 20000020, \
                     \"radius\": 999999999.0, \"position\": {\"x\": -100.0, \"y\": 200.0, \"z\": -300.0}, \
                     \"securityStatus\": 0.9459}\n",
                ),
                ("mapStargates.jsonl", ""),
                ("mapStars.jsonl", ""),
                ("mapPlanets.jsonl", ""),
                ("mapMoons.jsonl", ""),
                ("types.jsonl", ""),
            ],
        );
        let mut connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        let config = ParserConfig::default();
        let parser = Parser::new(&dir.path, config);

        // Doesn't fail -- the known Doomheim/InterBus-style case is
        // cleared to NULL automatically, not reported as an error.
        let summary = parser.parse_data(&mut connection).unwrap();
        assert_eq!(summary.npc_corporations, 1);

        let station_id: Option<i64> = connection
            .query_row(
                "SELECT stationId FROM npcCorporations WHERE corporationId = 1000001",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(station_id, None);
    }

    #[test]
    fn parse_data_rolls_back_everything_on_failure() {
        let dir = TempSdeDir::new(
            "parse_data_rollback",
            &[
                (
                    "categories.jsonl",
                    "{\"_key\": 6, \"name\": {\"en\": \"Celestial\"}, \"published\": true}\n",
                ),
                (
                    "groups.jsonl",
                    "{\"_key\": 6, \"categoryID\": 6, \"name\": {\"en\": \"Sun\"}, \"anchorable\": false}\n",
                ),
                (
                    "types.jsonl",
                    "{\"_key\": 3000, \"groupID\": 6, \"name\": {\"en\": \"Yellow G5 (ffcc00)\"}, \
                     \"iconID\": 100, \"published\": true, \"volume\": 0.0}\n",
                ),
                (
                    "races.jsonl",
                    "{\"_key\": 1, \"name\": {\"en\": \"Caldari\"}}\n",
                ),
                ("npcCorporationDivisions.jsonl", ""),
                (
                    "npcCorporations.jsonl",
                    "{\"_key\": 1000004, \"name\": {\"en\": \"CBD Corporation\"}, \
                     \"tickerName\": \"CBD\", \"deleted\": false, \"extent\": \"L\", \
                     \"hasPlayerPersonnelManager\": false, \"initialPrice\": 0, \"memberLimit\": -1, \
                     \"minSecurity\": 0.0, \"minimumJoinStanding\": 1, \
                     \"sendCharTerminationMessage\": true, \"shares\": 1000, \"size\": \"L\", \
                     \"taxRate\": 0.0, \"uniqueName\": true, \"iconID\": 500, \"raceID\": 1}\n",
                ),
                (
                    // sizeFactor is deliberately missing:
                    // factions.sizeFactor is REAL NOT NULL, so
                    // parse_factions() must fail.
                    "factions.jsonl",
                    "{\"_key\": 500001, \"name\": {\"en\": \"Caldari State\"}, \"iconID\": 600, \
                     \"uniqueName\": true, \"corporationID\": 1000004}\n",
                ),
            ],
        );
        let mut connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        let config = ParserConfig::default();
        let parser = Parser::new(&dir.path, config);

        let result = parser.parse_data(&mut connection);
        assert!(result.is_err());

        // Nothing should have been left persisted, not even the phases
        // before the one that failed (categories/groups/types/races/
        // npcCorporations had already been successfully inserted
        // before factions failed).
        for table in [
            "invCategories",
            "invGroups",
            "invTypes",
            "races",
            "npcCorporations",
            "factions",
            "factionRace",
            "typeStar",
        ] {
            let count: i64 = connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0, "table {table} should be empty after the rollback");
        }
    }

    // ---------------------------------------------------------------------
    // stationServices / stationOperations / npcStations
    // ---------------------------------------------------------------------
}
