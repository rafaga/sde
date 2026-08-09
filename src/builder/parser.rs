//! Populates the SDE data into the tables created by [`super::schema`].
//!
//! **Complete** port of `SdeParser`'s `_parse_*`/`parse_*` methods in
//! the Python prototype (`sde_parser.py`) -- see "Scope" below for the
//! phase-by-phase detail. Each Python method corresponds here to a
//! `parse_*` function that receives the already-open connection (with
//! the schema already created by [`super::schema::create_schema`]) and
//! the directory where the SDE's flat files live (`categories.jsonl`,
//! `types.jsonl`, etc.) -- with the exception of [`parse_connections`],
//! which doesn't read any file (it derives its data from
//! `mapSystemGates`, already inserted by an earlier phase).
//!
//! Unlike Python (where `SdeParser` is a class with mutable state on
//! `self`), here the functions are free and stateless; the only state
//! that genuinely needs sharing between two of them -- the "Sun" group's
//! id and the `typeId -> starTypeId` mapping detected by `parse_types()`
//! -- is passed explicitly via [`StarTypeState`].
//!
//! ## Scope of this file (phase 1 to phase 9 -- full parity)
//!
//! Covers the "base" tables the rest of the entities reference via FK
//! and that don't depend on any map table: `invCategories`,
//! `invGroups`, `invTypes` (including the special star-type detection
//! that feeds `typeStar`), `races` (phase 1), `npcCorporations` and
//! `factions` + `factionRace` (phase 2). Equivalent to
//! `_parse_categories`, `_parse_groups`, `_parse_types`, `_parse_races`,
//! `_parse_npc_corporations` and `_parse_factions` in Python.
//!
//! Phase 3 adds the first two map tables, without the
//! isometric/dimetric projection complexity `mapSolarSystems` brings:
//! `mapRegions` and `mapConstellations`. Equivalent to `_parse_regions`
//! and `_parse_constellations` in Python.
//!
//! Phase 4 adds `mapSolarSystems`, with the k-space/w-space/abyssal/void
//! scope filter ([`system_in_scope`]) and the isometric projection
//! ([`isometric_projection_2d`]). Equivalent to `_parse_solar_systems()`
//! in Python.
//!
//! Phase 5 adds `mapSystemGates` ([`parse_stargates`], gated by
//! `config.with_gates`), equivalent to `_parse_stargates()` in Python --
//! see its docstring for an important note on why this particular phase
//! needs to run inside an explicit transaction (it's not just an
//! atomicity concern, unlike the rest of the pipeline).
//!
//! Phase 6 adds `mapStars` ([`parse_stars`]), equivalent to
//! `_parse_stars()` in Python. `mapStars.jsonl`'s exact shape was
//! confirmed against a real data sample (not just against the Python
//! code, which at this point carried a note from the author themselves
//! warning that the shape hadn't been verified) -- see [`parse_stars`]'s
//! docstring for the detail.
//!
//! Phase 7 adds `mapPlanets` ([`parse_planets`]), equivalent to
//! `_parse_planets()` in Python -- same situation as `mapStars`, shape
//! confirmed against a real sample of 68407 records (see
//! [`parse_planets`]'s docstring).
//!
//! Phase 8 adds `mapMoons` ([`parse_moons`], gated by
//! `config.with_moons`), equivalent to `_parse_moons()` in Python.
//! Unlike `mapStars`/`mapPlanets`, there was NO real sample available
//! to verify against here (the real file weighs over 200 MiB) -- the
//! port relies solely on the Python code, whose docstring DOES confirm
//! the field list. See [`parse_moons`]'s docstring for the detail of
//! what remains an inference (not a verification) in this phase.
//!
//! Phase 9 adds `mapSystemConnections` ([`parse_connections`]),
//! equivalent to `parse_connections()` in Python -- the simplest of
//! all: a single SQL statement that derives the connections directly
//! from `mapSystemGates`, without reading any SDE file. With this,
//! `parser.rs` reaches full parity with Python's `parse_data()`.
//!
//! ## Supported file format
//!
//! JSONL only (`SdeConfig.file_format == 'jsonl'` in the Python
//! prototype, which is also the default). The prototype's YAML support
//! (the other branch of `_iter_records`) isn't ported -- adding it would
//! mean pulling in a new YAML-parsing dependency into the crate, a
//! decision that was deliberately left out of this phase.
//!
//! ## Known deviations from the Python prototype
//!
//! - `_parse_types()` declares a `process = {}` dict that's never
//!   filled in (`process.get(...)` always gives `None`), so its check
//!   `if process.get(...) is not None: pass` is never true and every
//!   type ends up inserted anyway -- it's dead code. This port doesn't
//!   replicate it; the observable behavior is identical (every type in
//!   the file gets inserted).
//! - If a "Sun"-group type's name doesn't have at least 3
//!   space-separated tokens, Python raises `IndexError` and aborts the
//!   whole process. Here, instead, that type simply isn't treated as a
//!   star (it isn't inserted into `typeStar`) and the rest of the file
//!   keeps processing normally. If strict parity is preferred (abort
//!   like Python), flag it to change this to a `BuilderError::Data`.
//! - Color extraction uses `strip_prefix('(')`/`strip_suffix(')')`
//!   instead of Python's blind `[1:-1]` slice (which strips the
//!   first/last character whether or not they're parentheses). With
//!   well-formed data the result is identical; with malformed data,
//!   this version is more tolerant.
//! - **Transactions**: in Python, no `_parse_*` does a `commit()` --
//!   the whole pipeline runs in a single implicit transaction that's
//!   only committed in `SdeParser.close()`, at the very end. If
//!   something fails partway through, nothing gets persisted (implicit
//!   rollback on closing without a commit). This file's individual
//!   `parse_*` functions, called on their own, do NOT wrap their
//!   inserts in an explicit transaction (autocommit per INSERT, SQLite's
//!   default mode) -- only [`parse_data`], the orchestrator, wraps all 6
//!   phases in a single real transaction (`Connection::transaction()`),
//!   with the same "all or nothing" behavior as Python. If
//!   `parse_categories()` (or another individual function) is called
//!   directly, outside of `parse_data`, the lack-of-atomicity described
//!   above still applies -- to get Python's guarantee you always have to
//!   go through `parse_data`.
//! - `_parse_factions()` iterates `faction.get('memberRaces', [])`
//!   without validating its elements -- if one weren't an integer,
//!   Python would just pass it to `cur.execute()` as-is and fail (or
//!   not) depending on the driver. Here, [`parse_factions`] validates
//!   each element and returns [`BuilderError::Data`] on the first one
//!   that isn't an integer, since it would violate
//!   `factionRace.raceId INTEGER NOT NULL` on insert anyway -- failing
//!   early with a clear message beats a generic SQLite error further
//!   down.
//! - `_parse_regions()` reads `region.get('nebulaID')` as optional, but
//!   `mapRegions.nebula` is `INTEGER NOT NULL` -- if it were missing,
//!   Python would fail the same way on insert (NOT NULL constraint).
//!   Here, [`parse_regions`] treats it as required (same criterion as
//!   `name` in phase 1): it fails with [`BuilderError::Data`] and a
//!   clear message instead of letting SQLite reject it further down.
//! - `_parse_regions()`/`_parse_constellations()` also build
//!   `self._region_names`/`self._constellation_names`, but only to
//!   compose the console progress-bar text (`print(...,
//!   end="\r")`) -- they don't affect any inserted data. This port
//!   doesn't replicate that cache, since there's no console-progress
//!   equivalent yet in any function in this file.
//! - `_parse_constellations()` computes the id as
//!   `element['constellationID'] if 'constellationID' in element else
//!   element['_key']`. Python distinguishes "the key is present" (with
//!   `in`) from "the value is valid"; if `constellationID` were present
//!   but `null` or some other type, Python would still use it (and
//!   probably fail further down on insert). [`parse_constellations`]
//!   instead falls back to `_key` in both cases (absent or
//!   present-but-not-an-integer) -- more tolerant, same result with
//!   well-formed data.
//! - Python's `_parse_solar_systems()` supports three algorithms for
//!   `projX`/`projY`/`projZ` depending on
//!   `self._config.projection_algorithm`: `'isometric'`
//!   (`calculate_isometric_projection()`), `'dimetric'`
//!   (`calculate_dimetric_projection()`, not ported) and any other
//!   value (raw passthrough of `position.x/y/z` untransformed, also not
//!   ported). Those three columns no longer exist in the schema (see
//!   below, "projX/Y/Z removed"), so this branch of Python isn't ported
//!   at all -- not even the `'isometric'` case, which used to be
//!   ported.
//! - **`projX`/`projY`/`projZ` removed from the schema**: they used to
//!   store a locally-computed 2D projection of the system's center (via
//!   [`isometric_projection_2d`]), separate from `position2DX`/
//!   `position2DY` (the 2D projection CCP already provides
//!   precomputed). Since both represent the same concept, keeping both
//!   was redundant -- the explicit decision was made to remove
//!   `projX/Y/Z` and migrate everything to `position2DX`/
//!   `position2DY` (including `SdeManager`'s queries in `src/lib.rs`,
//!   which used to read `projX`/`projZ`). [`isometric_projection_2d`]
//!   itself still exists -- it just lost this use case; it's still what
//!   computes `position2DX`/`position2DY` when
//!   `config.force_isometric_position_2d` is on.
//! - Python's `self._system_names` (populated in
//!   `_parse_solar_systems`, alongside `_systems_in_scope`) is never
//!   read anywhere in the prototype -- not even for a progress print,
//!   unlike `_region_names`/`_constellation_names`. It's pure dead
//!   code. This port doesn't replicate it.

use crate::builder::BuilderError;
use rusqlite::Connection;
use serde_json::Value;
use std::io::BufRead;
use std::path::Path;

/// Config for the parser. Covers what's needed for localizing names
/// (`_localized()` in Python) and the optional isometric computation of
/// `position2DX`/`position2DY` (see [`ProjectedAxis`]/
/// [`isometric_projection_2d`]). The solar-system scope flags
/// (k-space/w-space/abyssal/void) and the dimetric projection algorithm
/// (`calculate_dimetric_projection()` in Python, not ported) get added
/// when `_parse_solar_systems` gets ported in a future phase.
#[derive(Debug, Clone)]
pub struct ParserConfig {
    /// Language to extract from localized `name`/`description` fields
    /// (e.g. `{"en": "Jita", "es": "Jita"}` -> `"Jita"`), falling back
    /// to `"en"` if the requested language isn't there. Default `"en"`,
    /// same as `SdeConfig.language` in Python.
    pub language: String,
    /// If `true`, `position2DX`/`position2DY` are always computed
    /// locally via [`isometric_projection_2d`], **ignoring** the
    /// `position2D` field CCP already provides in the reworked SDE --
    /// instead of directly using the precomputed value CCP provides
    /// (which is the default behavior, `false`).
    ///
    /// Note: there's still no `parse_*` that populates
    /// `mapSolarSystems` (left for a future phase, see the module's
    /// docstring), so for now this flag has no observable effect -- it's
    /// the config piece that future function will consult.
    pub force_isometric_position_2d: bool,
    /// Axis collapsed in [`isometric_projection_2d`]'s computation when
    /// `force_isometric_position_2d` is on (no effect if it isn't).
    /// Default [`ProjectedAxis::Y`], same as
    /// `SdeConfig.projected_axis = 1` in Python (`0` for X, `1` for Y,
    /// `2` for Z).
    pub isometric_projected_axis: ProjectedAxis,
    /// Include k-space systems (no `wormholeClassID`). Default `true`,
    /// same as `SdeConfig.map_kspace` in Python.
    pub map_kspace: bool,
    /// Include wormhole space systems. Default `true`, same as
    /// `SdeConfig.map_wspace` in Python.
    pub map_wspace: bool,
    /// Include abyssal deadspace systems. Default `true`, same as
    /// `SdeConfig.map_abyssal` in Python.
    pub map_abyssal: bool,
    /// Include "void" systems. Default `false`, same as
    /// `SdeConfig.map_void` in Python. See [`system_in_scope`] for a
    /// note on why, today, `map_wspace`/`map_abyssal`/`map_void` all
    /// end up gating on the same check.
    pub map_void: bool,
    /// If `false`, [`parse_data`] skips the stargates phase
    /// ([`parse_stargates`]) entirely -- doesn't call it at all, not
    /// just filter its results. Default `true`, same as
    /// `SdeConfig.with_gates` in Python.
    pub with_gates: bool,
    /// If `false`, [`parse_data`] skips the moons phase
    /// ([`parse_moons`]) entirely -- doesn't call it at all. Default
    /// `true`, same as `SdeConfig.with_moons` in Python.
    pub with_moons: bool,
}

impl Default for ParserConfig {
    fn default() -> Self {
        Self {
            language: "en".to_string(),
            force_isometric_position_2d: false,
            isometric_projected_axis: ProjectedAxis::default(),
            map_kspace: true,
            map_wspace: true,
            map_abyssal: true,
            map_void: false,
            with_gates: true,
            with_moons: true,
        }
    }
}

/// Axis choice used by [`crate::objects::MapPoint::to_2d`] and
/// [`isometric_projection_2d`] -- moved to `crate::objects` (core, not
/// gated by the `builder` feature) since `MapPoint` needs it too, on
/// the read side. Re-exported here so existing `parser::ProjectedAxis`
/// references throughout this module keep working unchanged.
pub use crate::objects::ProjectedAxis;

/// 2D isometric projection of a 3D point, collapsing `axis`. Exact port
/// of `calculate_isometric_projection()` in Python (same formulas, same
/// collapsed axis); unlike Python, which always returns a 3-tuple with
/// a `0.0` filler on the collapsed axis, this function directly returns
/// the two non-null components as `(x2d, y2d)`.
///
/// Formulas (from <https://www.compuphase.com/axometr.htm>, per the
/// original comment in Python):
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

/// Decides whether a solar system should be imported, based on the
/// `map_kspace`/`map_wspace`/`map_abyssal`/`map_void` flags. Exact port
/// of `_system_in_scope()` in Python.
///
/// The reworked SDE no longer splits k-space/w-space/abyssal/void by
/// directory like the old one did; the only confirmed discriminator in
/// the record itself is `wormholeClassID` (only present in systems that
/// are NOT k-space). CCP doesn't expose a finer-grained flag to
/// distinguish abyssal from void at this level, so -- same as in
/// Python -- `map_wspace`/`map_abyssal`/`map_void` currently share the
/// same check ("does it have a `wormholeClassID`?").
fn system_in_scope(wormhole_class_id: Option<i64>, config: &ParserConfig) -> bool {
    match wormhole_class_id {
        None => config.map_kspace,
        Some(_) => config.map_wspace || config.map_abyssal || config.map_void,
    }
}

/// State shared between [`parse_groups`] and [`parse_types`], equivalent
/// to `SdeParser._stars` (`DataBrigde`) in Python.
#[derive(Debug, Default)]
pub struct StarTypeState {
    /// `groupId` of the group named exactly `"Sun"`, once
    /// [`parse_groups`] finds it.
    pub sun_group_id: Option<i64>,
    /// `typeId` (from `invTypes`) -> `starTypeId` (from `typeStar`) for
    /// each star type inserted by [`parse_types`]. Used by
    /// [`parse_stars`] to resolve each star's `starTypeId`.
    pub star_type_ids: std::collections::HashMap<i64, i64>,
}

/// Solar system ids that passed the [`system_in_scope`] filter,
/// populated by [`parse_solar_systems`]. Equivalent to
/// `self._systems_in_scope` in Python, which `_parse_stargates`,
/// `_parse_stars`, `_parse_planets` and `_parse_moons` use to filter
/// their own records by `solarSystemID`.
#[derive(Debug, Default)]
pub struct SystemScopeState {
    pub systems_in_scope: std::collections::HashSet<i64>,
}

// ---------------------------------------------------------------------
// Shared infrastructure: flat-file reading + field-extraction helpers,
// equivalent to `_iter_records()` / `_localized()` in Python.
// ---------------------------------------------------------------------

/// Iterates the records in `<sde_directory>/<stem>.jsonl`, one
/// non-empty line at a time, as [`serde_json::Value`].
///
/// Equivalent to the `jsonl` branch of `_iter_records()` in Python.
/// Each record carries its own `_key` field (the id) by convention of
/// the new SDE -- unlike Python's YAML branch, there's no need to
/// inject it separately.
fn iter_jsonl_records(
    sde_directory: &Path,
    stem: &str,
) -> Result<impl Iterator<Item = Result<Value, BuilderError>>, BuilderError> {
    let path = sde_directory.join(format!("{stem}.jsonl"));
    let file = std::fs::File::open(&path)?;
    let reader = std::io::BufReader::new(file);
    Ok(reader.lines().filter_map(|line| match line {
        Ok(line) if line.trim().is_empty() => None,
        Ok(line) => Some(serde_json::from_str::<Value>(&line).map_err(BuilderError::Json)),
        Err(err) => Some(Err(BuilderError::Io(err))),
    }))
}

/// Extracts the string localized to `config.language` from a field
/// shaped like `{"en": "...", "es": "...", ...}`, falling back to
/// `"en"`. If `field` is already a plain string (not an object), it's
/// returned as-is. Equivalent to `_localized()` in Python.
fn localized<'a>(record: &'a Value, field: &str, config: &ParserConfig) -> Option<&'a str> {
    match record.get(field) {
        Some(Value::Object(map)) => map
            .get(config.language.as_str())
            .or_else(|| map.get("en"))
            .and_then(Value::as_str),
        Some(Value::String(s)) => Some(s.as_str()),
        _ => None,
    }
}

/// Extracts a required integer field from the record. Equivalent to a
/// `dict[key]`-style access in Python (which raises `KeyError` if
/// missing): if the field isn't present or isn't numeric, this is a
/// data error ([`BuilderError::Data`]), not a silent `None`.
fn required_i64(record: &Value, field: &str) -> Result<i64, BuilderError> {
    record.get(field).and_then(Value::as_i64).ok_or_else(|| {
        BuilderError::Data(format!(
            "record missing required field `{field}` (or it's not an integer): {record}"
        ))
    })
}

/// Extracts an optional integer field. Equivalent to `dict.get(key)` in
/// Python (`None` if missing, no error).
fn optional_i64(record: &Value, field: &str) -> Option<i64> {
    record.get(field).and_then(Value::as_i64)
}

/// Extracts a required localized name (via [`localized`]); if the field
/// is missing or isn't a localizable string/object, this is a data
/// error. The name columns this feeds (`categoryName`, `groupName`,
/// `typeName`, `raceName`) are all `TEXT NOT NULL` in the STRICT
/// schema -- Python would also fail here (with an `IntegrityError` when
/// trying to insert `NULL`), so it's treated just as "hard" as
/// `required_i64` instead of silently inserting an empty string.
fn required_localized<'a>(
    record: &'a Value,
    field: &str,
    config: &ParserConfig,
) -> Result<&'a str, BuilderError> {
    localized(record, field, config).ok_or_else(|| {
        BuilderError::Data(format!(
            "record has no localizable field `{field}` in `{}`/`en`: {record}",
            config.language
        ))
    })
}

/// Extracts an optional boolean field. Equivalent to `dict.get(key)`.
fn optional_bool(record: &Value, field: &str) -> Option<bool> {
    record.get(field).and_then(Value::as_bool)
}

/// Extracts an optional floating-point field. Equivalent to
/// `dict.get(key)`.
fn optional_f64(record: &Value, field: &str) -> Option<f64> {
    record.get(field).and_then(Value::as_f64)
}

/// Extracts a required plain string field (not localized -- for fields
/// like `tickerName` that don't carry per-language variants). Equivalent
/// to a `dict[key]` access in Python.
fn required_str<'a>(record: &'a Value, field: &str) -> Result<&'a str, BuilderError> {
    record.get(field).and_then(Value::as_str).ok_or_else(|| {
        BuilderError::Data(format!(
            "record missing required field `{field}` (or it's not a string): {record}"
        ))
    })
}

/// Extracts a required boolean field. Equivalent to a `dict[key]`
/// access in Python.
fn required_bool(record: &Value, field: &str) -> Result<bool, BuilderError> {
    record.get(field).and_then(Value::as_bool).ok_or_else(|| {
        BuilderError::Data(format!(
            "record missing required field `{field}` (or it's not a boolean): {record}"
        ))
    })
}

/// Extracts a required floating-point field. Equivalent to a
/// `dict[key]` access in Python.
fn required_f64(record: &Value, field: &str) -> Result<f64, BuilderError> {
    record.get(field).and_then(Value::as_f64).ok_or_else(|| {
        BuilderError::Data(format!(
            "record missing required field `{field}` (or it's not a number): {record}"
        ))
    })
}

/// Extracts ids from an optional integer array -- empty if the field is
/// missing or `null`, same as `faction.get('memberRaces', [])` in
/// Python. If the field IS present but isn't an array, or any of its
/// elements isn't an integer, that's a data error (see "Known
/// deviations" in the module's docstring).
fn optional_i64_array(record: &Value, field: &str) -> Result<Vec<i64>, BuilderError> {
    match record.get(field) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_i64().ok_or_else(|| {
                    BuilderError::Data(format!("non-integer element in array `{field}`: {item}"))
                })
            })
            .collect(),
        Some(other) => Err(BuilderError::Data(format!(
            "field `{field}` is not an array: {other}"
        ))),
    }
}

/// Extracts `record["position"]["x"/"y"/"z"]` as `(f64, f64, f64)`.
/// Equivalent to the nested access `record['position']['x']` (etc.) in
/// Python -- both levels are required (`dict[key]`, not `.get()`); if
/// `position` or any of its three components is missing, that's a data
/// error.
fn required_position(record: &Value) -> Result<(f64, f64, f64), BuilderError> {
    let position = record.get("position").ok_or_else(|| {
        BuilderError::Data(format!(
            "record missing required field `position`: {record}"
        ))
    })?;
    let x = required_f64(position, "x")?;
    let y = required_f64(position, "y")?;
    let z = required_f64(position, "z")?;
    Ok((x, y, z))
}

/// Extracts `record[outer][inner]` as a required `i64`. Equivalent to
/// the nested access `record[outer][inner]` in Python (both levels are
/// `dict[key]`, not `.get()`) -- used for `destination.stargateID`/
/// `destination.solarSystemID` in [`parse_stargates`].
fn required_nested_i64(record: &Value, outer: &str, inner: &str) -> Result<i64, BuilderError> {
    let outer_val = record.get(outer).ok_or_else(|| {
        BuilderError::Data(format!("record missing required field `{outer}`: {record}"))
    })?;
    required_i64(outer_val, inner)
}

/// Extracts an optional integer field that can either be at the
/// record's top level or nested under `nested_field` (e.g.
/// `statistics`), with the top level taking priority. Approximates
/// Python's `record.get(field, nested.get(field))` pattern (where
/// `nested = record.get(nested_field) or {}`), used in `_parse_stars()`
/// for `radius`/`locked` -- with a minor difference: Python
/// distinguishes "the key is present but is `null`" (doesn't fall
/// through to nested) from "the key is absent" (does fall through);
/// here both cases fall through to nested alike, since `optional_i64`
/// doesn't distinguish "absent" from "present but of the wrong
/// type/null".
fn optional_i64_with_nested_fallback(
    record: &Value,
    field: &str,
    nested_field: &str,
) -> Option<i64> {
    optional_i64(record, field).or_else(|| {
        record
            .get(nested_field)
            .and_then(|nested| optional_i64(nested, field))
    })
}

/// Same as [`optional_i64_with_nested_fallback`], but for boolean
/// fields (e.g. `locked`).
fn optional_bool_with_nested_fallback(
    record: &Value,
    field: &str,
    nested_field: &str,
) -> Option<bool> {
    optional_bool(record, field).or_else(|| {
        record
            .get(nested_field)
            .and_then(|nested| optional_bool(nested, field))
    })
}

/// Same as [`optional_i64_with_nested_fallback`], but for floating-point
/// fields -- used for `mapPlanets.radius` (a `REAL` column, unlike
/// `mapStars.radius`, which is `INTEGER`).
fn optional_f64_with_nested_fallback(
    record: &Value,
    field: &str,
    nested_field: &str,
) -> Option<f64> {
    optional_f64(record, field).or_else(|| {
        record
            .get(nested_field)
            .and_then(|nested| optional_f64(nested, field))
    })
}

/// Extracts an optional plain string field. Equivalent to
/// `dict.get(key)` in Python (`None` if missing, no error).
fn optional_str<'a>(record: &'a Value, field: &str) -> Option<&'a str> {
    record.get(field).and_then(Value::as_str)
}

/// Extracts `record[outer][inner]` as `f64`, returning `None` if either
/// level is missing (or isn't numeric). Equivalent to the pattern
/// `outer_val = record.get(outer); outer_val.get(inner) if outer_val
/// else None` in Python -- used for `position2D.x`/`.y`, which, unlike
/// `position` (see [`required_position`]), is optional at both levels.
fn optional_nested_f64(record: &Value, outer: &str, inner: &str) -> Option<f64> {
    record.get(outer)?.get(inner).and_then(Value::as_f64)
}

// ---------------------------------------------------------------------
// invCategories
// ---------------------------------------------------------------------

/// Populates `invCategories` from `<sde_directory>/categories.jsonl`.
/// Returns the number of rows inserted. Equivalent to
/// `_parse_categories()` in Python.
pub fn parse_categories(
    connection: &Connection,
    sde_directory: &Path,
    config: &ParserConfig,
) -> Result<usize, BuilderError> {
    let mut insert_category = connection.prepare(
        "INSERT INTO invCategories (categoryId, categoryName, published) VALUES (?1, ?2, ?3)",
    )?;

    let mut count = 0usize;
    for record in iter_jsonl_records(sde_directory, "categories")? {
        let record = record?;
        let id = required_i64(&record, "_key")?;
        let name = required_localized(&record, "name", config)?;
        let published = optional_bool(&record, "published");

        insert_category.execute(rusqlite::params![id, name, published])?;
        count += 1;
    }
    Ok(count)
}

// ---------------------------------------------------------------------
// invGroups
// ---------------------------------------------------------------------

/// Populates `invGroups` from `<sde_directory>/groups.jsonl`. Along the
/// way, detects the group named exactly `"Sun"` and saves its id in
/// `state.sun_group_id` -- [`parse_types`] needs it to recognize star
/// types. Returns the number of rows inserted. Equivalent to
/// `_parse_groups()` in Python.
pub fn parse_groups(
    connection: &Connection,
    sde_directory: &Path,
    config: &ParserConfig,
    state: &mut StarTypeState,
) -> Result<usize, BuilderError> {
    let mut insert_group = connection.prepare(
        "INSERT INTO invGroups (groupId, categoryId, groupName, anchorable) \
         VALUES (?1, ?2, ?3, ?4)",
    )?;

    let mut count = 0usize;
    for record in iter_jsonl_records(sde_directory, "groups")? {
        let record = record?;
        let id = required_i64(&record, "_key")?;
        let category_id = required_i64(&record, "categoryID")?;
        let name = required_localized(&record, "name", config)?;
        let anchorable = optional_bool(&record, "anchorable");

        insert_group.execute(rusqlite::params![id, category_id, name, anchorable])?;

        if name == "Sun" {
            state.sun_group_id = Some(id);
        }

        count += 1;
    }
    Ok(count)
}

// ---------------------------------------------------------------------
// invTypes (+ typeStar for star types)
// ---------------------------------------------------------------------

/// Inserts a row into `typeStar` and returns the `starTypeId` SQLite
/// assigned it. Equivalent to `add_star_type()` in Python (which does
/// the same thing: INSERT and then a SELECT to read it back by
/// `typeId`, since `typeStar.starTypeId` has no `AUTOINCREMENT` -- it's
/// a plain `ROWID` that still gets auto-assigned).
fn add_star_type(
    connection: &Connection,
    type_id: i64,
    name: &str,
    color: &str,
) -> Result<i64, BuilderError> {
    connection.execute(
        "INSERT INTO typeStar (typeId, name, color) VALUES (?1, ?2, ?3)",
        rusqlite::params![type_id, name, color],
    )?;
    let star_type_id = connection.query_row(
        "SELECT starTypeId FROM typeStar WHERE typeId = ?1",
        rusqlite::params![type_id],
        |row| row.get(0),
    )?;
    Ok(star_type_id)
}

/// Populates `invTypes` from `<sde_directory>/types.jsonl`, and along
/// the way `typeStar` for any type belonging to the "Sun" group (detected
/// by [`parse_groups`] via `state.sun_group_id`). Returns the number of
/// rows inserted into `invTypes`. Equivalent to `_parse_types()` in
/// Python -- see "Known deviations" in the module's docstring for how
/// the dead `process` code and malformed star names are handled.
pub fn parse_types(
    connection: &Connection,
    sde_directory: &Path,
    config: &ParserConfig,
    state: &mut StarTypeState,
) -> Result<usize, BuilderError> {
    let mut insert_type = connection.prepare(
        "INSERT INTO invTypes (typeId, groupId, typeName, iconId, published, volume) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?;

    let mut count = 0usize;
    for record in iter_jsonl_records(sde_directory, "types")? {
        let record = record?;
        let id = required_i64(&record, "_key")?;
        let group_id = required_i64(&record, "groupID")?;
        let name = required_localized(&record, "name", config)?.to_string();
        let icon_id = optional_i64(&record, "iconID");
        let published = optional_bool(&record, "published");
        let volume = optional_f64(&record, "volume");

        insert_type.execute(rusqlite::params![
            id, group_id, name, icon_id, published, volume
        ])?;

        if state.sun_group_id == Some(group_id) {
            let parts: Vec<&str> = name.split(' ').collect();
            if parts.len() >= 3 {
                let star_name = parts[1];
                let color_token = parts[2];
                let color = color_token
                    .strip_prefix('(')
                    .and_then(|s| s.strip_suffix(')'))
                    .unwrap_or(color_token);
                let star_type_id = add_star_type(connection, id, star_name, color)?;
                state.star_type_ids.insert(id, star_type_id);
            }
            // Fewer than 3 tokens: not treated as a star. See "Known
            // deviations" in the module's docstring -- Python would
            // abort the whole process with an IndexError here.
        }

        count += 1;
    }
    Ok(count)
}

// ---------------------------------------------------------------------
// races
// ---------------------------------------------------------------------

/// Populates `races` from `<sde_directory>/races.jsonl`. Returns the
/// number of rows inserted. Equivalent to `_parse_races()` in Python.
pub fn parse_races(
    connection: &Connection,
    sde_directory: &Path,
    config: &ParserConfig,
) -> Result<usize, BuilderError> {
    let mut insert_race =
        connection.prepare("INSERT INTO races (raceId, raceName) VALUES (?1, ?2)")?;

    let mut count = 0usize;
    for record in iter_jsonl_records(sde_directory, "races")? {
        let record = record?;
        let id = required_i64(&record, "_key")?;
        let name = required_localized(&record, "name", config)?;

        insert_race.execute(rusqlite::params![id, name])?;
        count += 1;
    }
    Ok(count)
}

// ---------------------------------------------------------------------
// npcCorporationDivisions
// ---------------------------------------------------------------------

/// Populates `npcCorporationDivisions` from
/// `<sde_directory>/npcCorporationDivisions.jsonl` (10 records,
/// confirmed complete for `_key`/`internalName`/`leaderTypeName`).
/// No equivalent in the Python prototype -- added directly against the
/// real SDE export, same as `npcStations`'s subsystem (see that
/// function's docstring).
pub fn parse_npc_corporation_divisions(
    connection: &Connection,
    sde_directory: &Path,
    config: &ParserConfig,
) -> Result<usize, BuilderError> {
    let mut insert = connection.prepare(
        "INSERT INTO npcCorporationDivisions (divisionId, internalName, leaderTypeName) \
         VALUES (?1, ?2, ?3)",
    )?;

    let mut count = 0usize;
    for record in iter_jsonl_records(sde_directory, "npcCorporationDivisions")? {
        let record = record?;
        let id = required_i64(&record, "_key")?;
        let internal_name = required_str(&record, "internalName")?;
        let leader_type_name = required_localized(&record, "leaderTypeName", config)?;
        insert.execute(rusqlite::params![id, internal_name, leader_type_name])?;
        count += 1;
    }
    Ok(count)
}

// ---------------------------------------------------------------------
// npcCorporations
// ---------------------------------------------------------------------

/// Populates `npcCorporations`, `npcCorporationAllowedRaces`,
/// `npcCorporationDivisionAssignments`, `npcCorporationTrades`, and
/// `npcCorporationInvestors` from
/// `<sde_directory>/npcCorporations.jsonl`. Requires `races` and
/// [`parse_npc_corporation_divisions`] to already be populated.
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
pub fn parse_npc_corporations(
    connection: &Connection,
    sde_directory: &Path,
    config: &ParserConfig,
) -> Result<usize, BuilderError> {
    let mut insert_corp = connection.prepare(
        "INSERT INTO npcCorporations \
         (corporationId, corporationName, tickerName, deleted, description, extent, \
          hasPlayerPersonnelManager, initialPrice, memberLimit, minSecurity, minimumJoinStanding, \
          sendCharTerminationMessage, shares, size, sizeFactor, taxRate, uniqueName, ceoId, \
          mainActivityId, secondaryActivityId, iconId, raceId, enemyId, friendId, factionId, \
          solarSystemId, stationId) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, \
                  ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27)",
    )?;
    let mut insert_allowed_race = connection.prepare(
        "INSERT INTO npcCorporationAllowedRaces (corporationId, raceId) VALUES (?1, ?2)",
    )?;
    let mut insert_division = connection.prepare(
        "INSERT INTO npcCorporationDivisionAssignments \
         (corporationId, divisionId, divisionNumber, leaderId, size) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;
    let mut insert_trade = connection.prepare(
        "INSERT INTO npcCorporationTrades (corporationId, typeId, affinity) VALUES (?1, ?2, ?3)",
    )?;
    let mut insert_investor = connection.prepare(
        "INSERT INTO npcCorporationInvestors (corporationId, investorId, shares) VALUES (?1, ?2, ?3)",
    )?;

    let mut count = 0usize;
    for record in iter_jsonl_records(sde_directory, "npcCorporations")? {
        let record = record?;
        let id = required_i64(&record, "_key")?;
        let name = required_localized(&record, "name", config)?;
        let ticker = required_str(&record, "tickerName")?;
        let deleted = required_bool(&record, "deleted")?;
        let description = localized(&record, "description", config);
        let extent = required_str(&record, "extent")?;
        let has_player_personnel_manager = required_bool(&record, "hasPlayerPersonnelManager")?;
        let initial_price = required_i64(&record, "initialPrice")?;
        let member_limit = required_i64(&record, "memberLimit")?;
        let min_security = required_f64(&record, "minSecurity")?;
        let minimum_join_standing = required_f64(&record, "minimumJoinStanding")?;
        let send_char_termination_message = required_bool(&record, "sendCharTerminationMessage")?;
        let shares = required_i64(&record, "shares")?;
        let size = required_str(&record, "size")?;
        let size_factor = optional_f64(&record, "sizeFactor");
        let tax_rate = required_f64(&record, "taxRate")?;
        let unique_name = required_bool(&record, "uniqueName")?;
        let ceo_id = optional_i64(&record, "ceoID");
        let main_activity_id = optional_i64(&record, "mainActivityID");
        let secondary_activity_id = optional_i64(&record, "secondaryActivityID");
        let icon_id = optional_i64(&record, "iconID");
        let race_id = optional_i64(&record, "raceID");
        let enemy_id = optional_i64(&record, "enemyID");
        let friend_id = optional_i64(&record, "friendID");
        let faction_id = optional_i64(&record, "factionID");
        let solar_system_id = optional_i64(&record, "solarSystemID");
        let station_id = optional_i64(&record, "stationID");

        insert_corp.execute(rusqlite::params![
            id,
            name,
            ticker,
            deleted,
            description,
            extent,
            has_player_personnel_manager,
            initial_price,
            member_limit,
            min_security,
            minimum_join_standing,
            send_char_termination_message,
            shares,
            size,
            size_factor,
            tax_rate,
            unique_name,
            ceo_id,
            main_activity_id,
            secondary_activity_id,
            icon_id,
            race_id,
            enemy_id,
            friend_id,
            faction_id,
            solar_system_id,
            station_id
        ])?;

        for allowed_race_id in optional_i64_array(&record, "allowedMemberRaces")? {
            insert_allowed_race.execute(rusqlite::params![id, allowed_race_id])?;
        }

        if let Some(Value::Array(divisions)) = record.get("divisions") {
            for entry in divisions {
                let division_id = required_i64(entry, "_key")?;
                let division_number = required_i64(entry, "divisionNumber")?;
                let leader_id = required_i64(entry, "leaderID")?;
                let division_size = required_i64(entry, "size")?;
                insert_division.execute(rusqlite::params![
                    id,
                    division_id,
                    division_number,
                    leader_id,
                    division_size
                ])?;
            }
        }

        if let Some(Value::Array(trades)) = record.get("corporationTrades") {
            for entry in trades {
                let type_id = required_i64(entry, "_key")?;
                let affinity = required_f64(entry, "_value")?;
                insert_trade.execute(rusqlite::params![id, type_id, affinity])?;
            }
        }

        if let Some(Value::Array(investors)) = record.get("investors") {
            for entry in investors {
                let investor_id = required_i64(entry, "_key")?;
                let investor_shares = required_f64(entry, "_value")?;
                insert_investor.execute(rusqlite::params![id, investor_id, investor_shares])?;
            }
        }

        count += 1;
    }
    Ok(count)
}

// ---------------------------------------------------------------------
// factions (+ factionRace)
// ---------------------------------------------------------------------

/// Populates `factions` and `factionRace` from
/// `<sde_directory>/factions.jsonl`. Requires `npcCorporations` to
/// already be populated if any record carries `corporationID`/
/// `militiaCorporationID`, and `races` for any id in `memberRaces`.
/// `solarSystemId` is `DEFERRABLE` (`mapSolarSystems` isn't parsed
/// until phase 4, after this phase 2). Returns the number of factions
/// inserted (doesn't count `factionRace` rows).
///
/// Rewritten against a real 27-record sample (August 2026) --
/// `description`/`solarSystemID` are new fields, both present in 100%
/// of real records but not previously captured at all.
/// `shortDescription`/`flatLogo`/`flatLogoWithName`/
/// `militiaCorporationID` are rarer (14.8%/66.7%/22.2%/22.2%) but
/// genuinely present.
pub fn parse_factions(
    connection: &Connection,
    sde_directory: &Path,
    config: &ParserConfig,
) -> Result<usize, BuilderError> {
    let mut insert_faction = connection.prepare(
        "INSERT INTO factions \
         (factionId, factionName, iconId, sizeFactor, uniqueName, description, shortDescription, \
          flatLogo, flatLogoWithName, corporationId, militiaCorporationId, solarSystemId) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
    )?;
    let mut insert_faction_race =
        connection.prepare("INSERT INTO factionRace (factionId, raceId) VALUES (?1, ?2)")?;

    let mut count = 0usize;
    for record in iter_jsonl_records(sde_directory, "factions")? {
        let record = record?;
        let id = required_i64(&record, "_key")?;
        let name = required_localized(&record, "name", config)?;
        let icon_id = required_i64(&record, "iconID")?;
        let size_factor = required_f64(&record, "sizeFactor")?;
        let unique_name = required_bool(&record, "uniqueName")?;
        let description = required_localized(&record, "description", config)?;
        let short_description = localized(&record, "shortDescription", config);
        let flat_logo = optional_str(&record, "flatLogo");
        let flat_logo_with_name = optional_str(&record, "flatLogoWithName");
        let corporation_id = optional_i64(&record, "corporationID");
        let militia_corporation_id = optional_i64(&record, "militiaCorporationID");
        let solar_system_id = optional_i64(&record, "solarSystemID");
        let member_races = optional_i64_array(&record, "memberRaces")?;

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

        count += 1;
    }
    Ok(count)
}

// ---------------------------------------------------------------------
// mapRegions
// ---------------------------------------------------------------------

/// Populates `mapRegions` from `<sde_directory>/mapRegions.jsonl`.
/// Returns the number of rows inserted. Equivalent to
/// `_parse_regions()` in Python.
///
/// `maxProjX`/`maxProjY` aren't included in the INSERT: the DDL gives
/// them `DEFAULT(0.0)` and Python doesn't specify them in its own query
/// either, so SQLite applies that default automatically in both cases.
pub fn parse_regions(
    connection: &Connection,
    sde_directory: &Path,
    config: &ParserConfig,
) -> Result<usize, BuilderError> {
    let mut insert_region = connection.prepare(
        "INSERT INTO mapRegions (regionId, regionName, factionId, centerX, centerY, centerZ, nebula, wormholeClassId) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )?;

    let mut count = 0usize;
    for record in iter_jsonl_records(sde_directory, "mapRegions")? {
        let record = record?;
        let id = required_i64(&record, "_key")?;
        let name = required_localized(&record, "name", config)?;
        let faction_id = optional_i64(&record, "factionID");
        let nebula = required_i64(&record, "nebulaID")?;
        let wormhole_class_id = optional_i64(&record, "wormholeClassID");
        let (center_x, center_y, center_z) = required_position(&record)?;

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
        count += 1;
    }
    Ok(count)
}

// ---------------------------------------------------------------------
// mapConstellations
// ---------------------------------------------------------------------

/// Populates `mapConstellations` from
/// `<sde_directory>/mapConstellations.jsonl`. Requires `mapRegions` to
/// already be populated (FK `mapConstellations.regionId ->
/// mapRegions.regionId`). Returns the number of rows inserted.
/// Equivalent to `_parse_constellations()` in Python.
///
/// The preferred id is `constellationID` if the record carries it; if
/// not, it falls back to `_key` -- replicates Python's
/// `element['constellationID'] if 'constellationID' in element else
/// element['_key']` (see "Known deviations" in the module's docstring
/// for the nuance of when this differs).
pub fn parse_constellations(
    connection: &Connection,
    sde_directory: &Path,
    config: &ParserConfig,
) -> Result<usize, BuilderError> {
    let mut insert_constellation = connection.prepare(
        "INSERT INTO mapConstellations (constellationId, constellationName, regionId, centerX, centerY, centerZ) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?;

    let mut count = 0usize;
    for record in iter_jsonl_records(sde_directory, "mapConstellations")? {
        let record = record?;
        let id = match optional_i64(&record, "constellationID") {
            Some(id) => id,
            None => required_i64(&record, "_key")?,
        };
        let name = required_localized(&record, "name", config)?;
        let region_id = required_i64(&record, "regionID")?;
        let (center_x, center_y, center_z) = required_position(&record)?;

        insert_constellation.execute(rusqlite::params![
            id, name, region_id, center_x, center_y, center_z
        ])?;
        count += 1;
    }
    Ok(count)
}

// ---------------------------------------------------------------------
// mapSolarSystems
// ---------------------------------------------------------------------

/// Populates `mapSolarSystems` from
/// `<sde_directory>/mapSolarSystems.jsonl`, filtering by
/// [`system_in_scope`] and accumulating the ids that pass the filter
/// into `state.systems_in_scope`. Requires `mapConstellations` to
/// already be populated (FK `mapSolarSystems.constellationId ->
/// mapConstellations.constellationId`). Returns the number of rows
/// inserted (out-of-scope systems do NOT count). Equivalent to
/// `_parse_solar_systems()` in Python.
///
/// `projX`/`projY`/`projZ` no longer exist in the schema (they were
/// removed: those columns' only real purpose was storing a 2D
/// projection of the system's center, and that's exactly what
/// `position2DX`/`position2DY` already does -- keeping both was
/// redundant). See `schema.sql` and `SdeManager` in `src/lib.rs`, which
/// was migrated to read `position2DX`/`position2DY` instead of
/// `projX`/`projZ`.
///
/// `position2DX`/`position2DY` use the `position2D` CCP already
/// provides precomputed, unless `config.force_isometric_position_2d`
/// is on -- in which case they're always recomputed via
/// [`isometric_projection_2d`] (per
/// `config.isometric_projected_axis`), **ignoring** CCP's value, as was
/// explicitly decided for this flag (see its docstring in
/// [`ParserConfig`]).
pub fn parse_solar_systems(
    connection: &Connection,
    sde_directory: &Path,
    config: &ParserConfig,
    state: &mut SystemScopeState,
) -> Result<usize, BuilderError> {
    let mut insert_system = connection.prepare(
        "INSERT INTO mapSolarSystems (solarSystemId, solarSystemName, constellationId, \
         corridor, fringe, hub, international, luminosity, radius, centerX, centerY, centerZ, \
         regional, security, securityClass, position2DX, position2DY) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
    )?;

    let mut count = 0usize;
    for record in iter_jsonl_records(sde_directory, "mapSolarSystems")? {
        let record = record?;
        let system_id = required_i64(&record, "_key")?;
        let wormhole_class_id = optional_i64(&record, "wormholeClassID");
        if !system_in_scope(wormhole_class_id, config) {
            continue;
        }
        state.systems_in_scope.insert(system_id);

        let name = required_localized(&record, "name", config)?;
        let constellation_id = required_i64(&record, "constellationID")?;
        let corridor = optional_bool(&record, "corridor");
        let fringe = optional_bool(&record, "fringe");
        let hub = optional_bool(&record, "hub");
        let international = optional_bool(&record, "international");
        let luminosity = optional_f64(&record, "luminosity");
        let radius = required_f64(&record, "radius")?;
        let (center_x, center_y, center_z) = required_position(&record)?;

        let regional = optional_bool(&record, "regional");
        let security = required_f64(&record, "securityStatus")?;
        let security_class = optional_str(&record, "securityClass");

        let (position_2d_x, position_2d_y) = if config.force_isometric_position_2d {
            let (x2d, y2d) = isometric_projection_2d(
                center_x,
                center_y,
                center_z,
                config.isometric_projected_axis,
            );
            (Some(x2d), Some(y2d))
        } else {
            (
                optional_nested_f64(&record, "position2D", "x"),
                optional_nested_f64(&record, "position2D", "y"),
            )
        };

        insert_system.execute(rusqlite::params![
            system_id,
            name,
            constellation_id,
            corridor,
            fringe,
            hub,
            international,
            luminosity,
            radius,
            center_x,
            center_y,
            center_z,
            regional,
            security,
            security_class,
            position_2d_x,
            position_2d_y,
        ])?;
        count += 1;
    }
    Ok(count)
}

// ---------------------------------------------------------------------
// mapSystemGates
// ---------------------------------------------------------------------

/// Populates `mapSystemGates` from `<sde_directory>/mapStargates.jsonl`
/// (the file is named `mapStargates`, even though the destination table
/// is `mapSystemGates` -- that's how the SDE itself names it). Filters
/// by `state.systems_in_scope` (populated by [`parse_solar_systems`]): a
/// gate whose `solarSystemID` isn't in that set is skipped -- same
/// criterion as `gate['solarSystemID'] not in self._systems_in_scope`
/// in Python. Requires `mapSolarSystems`/`invTypes` to already be
/// populated (FKs). Returns the number of rows inserted. Equivalent to
/// `_parse_stargates()` in Python.
///
/// # Important: requires an explicit transaction
///
/// `mapSystemGates.destinationGateId` references another row of the
/// SAME table (`systemGateId`), declared `DEFERRABLE INITIALLY
/// DEFERRED` in the schema -- that lets SQLite postpone that FK's
/// validation until the transaction's `COMMIT`, instead of requiring
/// the destination gate to already exist at the exact moment of the
/// INSERT. This matters because stargates usually come in pairs that
/// reference each other mutually (A's gate points to B's, and vice
/// versa), so whatever order the file is in, the first of the two to be
/// inserted necessarily references one that doesn't exist yet.
///
/// Verified empirically (sqlite3 with `isolation_level=None`, which
/// replicates SQLite/rusqlite's real autocommit mode): inserting that
/// first gate **outside** an explicit transaction fails with
/// `FOREIGN KEY constraint failed` -- in autocommit mode each `INSERT`
/// is its own implicit transaction, so the deferred validation still
/// fires immediately, when that single statement's transaction closes.
/// Wrapped in an explicit transaction (`BEGIN`/`COMMIT`), on the other
/// hand, both INSERTs resolve correctly because validation is postponed
/// until the final `COMMIT`, by which point both gates already exist.
///
/// In practice this means calling this function on its own (outside of
/// [`parse_data`], without going through `Connection::transaction()`)
/// doesn't just lose the "all or nothing" atomicity guarantee already
/// documented for the rest of the pipeline (see "Transactions" in the
/// module's docstring) -- here it can make the insertion of perfectly
/// valid data fail, purely because of the order records appear in the
/// file.
pub fn parse_stargates(
    connection: &Connection,
    sde_directory: &Path,
    state: &SystemScopeState,
) -> Result<usize, BuilderError> {
    let mut insert_gate = connection.prepare(
        "INSERT INTO mapSystemGates (systemGateId, solarSystemId, typeId, \
         positionX, positionY, positionZ, destinationGateId, destinationSystemId) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )?;

    let mut count = 0usize;
    for record in iter_jsonl_records(sde_directory, "mapStargates")? {
        let record = record?;
        let solar_system_id = required_i64(&record, "solarSystemID")?;
        if !state.systems_in_scope.contains(&solar_system_id) {
            continue;
        }

        let id = required_i64(&record, "_key")?;
        let type_id = required_i64(&record, "typeID")?;
        let (pos_x, pos_y, pos_z) = required_position(&record)?;
        let destination_gate_id = required_nested_i64(&record, "destination", "stargateID")?;
        let destination_system_id = required_nested_i64(&record, "destination", "solarSystemID")?;

        insert_gate.execute(rusqlite::params![
            id,
            solar_system_id,
            type_id,
            pos_x,
            pos_y,
            pos_z,
            destination_gate_id,
            destination_system_id,
        ])?;
        count += 1;
    }
    Ok(count)
}

// ---------------------------------------------------------------------
// mapStars
// ---------------------------------------------------------------------

/// Populates `mapStars` from `<sde_directory>/mapStars.jsonl`, filtering
/// by `state.systems_in_scope` (populated by [`parse_solar_systems`]).
/// Requires [`parse_types`] to have already run -- it needs
/// `star_state.star_type_ids`, the `typeId -> starTypeId` mapping --
/// and `mapSolarSystems`/`typeStar` to already be populated (FKs).
/// Returns the number of rows inserted. Equivalent to `_parse_stars()`
/// in Python.
///
/// Confirmed against a real sample of `mapStars.jsonl` (8089
/// records, EVE Online, August 2026): `radius` always comes at the
/// top level as an integer (never needs the nested fallback to
/// `statistics.radius`), `statistics` is always present, and `locked`
/// **never** shows up -- neither at the top level nor inside
/// `statistics` -- so in practice that column always comes out
/// `NULL`. The nested fallback (see [`optional_i64_with_nested_fallback`]/
/// [`optional_bool_with_nested_fallback`]) is kept anyway, faithfully
/// ported from Python, in case some other SDE version does carry it.
///
/// # Deviation from Python: `starTypeId` not found
///
/// Python resolves the star type with
/// `self._stars.entity_type.get(star['typeID'], star['typeID'])`: if
/// the star's `typeID` isn't in the map (meaning `_parse_types()`
/// didn't detect it as belonging to the "Sun" group), it uses the RAW
/// `typeID` as if it were a `starTypeId` -- almost certainly violating
/// the `mapStars.starTypeId -> typeStar.starTypeId` FK on insert, since
/// these are completely different id sequences (one is
/// `invTypes.typeId`, the other a self-assigned `ROWID` from
/// `typeStar`). Here, instead, not finding the `typeId` in the map is a
/// direct [`BuilderError::Data`] -- same criterion as the rest of this
/// file: fail early with a clear message instead of letting SQLite
/// reject a value that was going to be invalid anyway.
pub fn parse_stars(
    connection: &Connection,
    sde_directory: &Path,
    state: &SystemScopeState,
    star_state: &StarTypeState,
) -> Result<usize, BuilderError> {
    let mut insert_star = connection.prepare(
        "INSERT INTO mapStars (starId, solarSystemId, locked, radius, starTypeId) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;

    let mut count = 0usize;
    for record in iter_jsonl_records(sde_directory, "mapStars")? {
        let record = record?;
        let solar_system_id = required_i64(&record, "solarSystemID")?;
        if !state.systems_in_scope.contains(&solar_system_id) {
            continue;
        }

        let star_id = required_i64(&record, "_key")?;
        let locked = optional_bool_with_nested_fallback(&record, "locked", "statistics");
        let radius = optional_i64_with_nested_fallback(&record, "radius", "statistics");
        let type_id = required_i64(&record, "typeID")?;
        let star_type_id = star_state
            .star_type_ids
            .get(&type_id)
            .copied()
            .ok_or_else(|| {
                BuilderError::Data(format!(
                    "star {star_id}: typeId {type_id} isn't in star_type_ids \
                 (parse_types() didn't detect it as a star type)"
                ))
            })?;

        insert_star.execute(rusqlite::params![
            star_id,
            solar_system_id,
            locked,
            radius,
            star_type_id
        ])?;
        count += 1;
    }
    Ok(count)
}

// ---------------------------------------------------------------------
// mapPlanets
// ---------------------------------------------------------------------

/// Populates `mapPlanets` from `<sde_directory>/mapPlanets.jsonl`,
/// filtering by `state.systems_in_scope` (populated by
/// [`parse_solar_systems`]). Requires `mapSolarSystems`/`invTypes` to
/// already be populated (FKs). Returns the number of rows inserted.
/// Equivalent to `_parse_planets()` in Python.
///
/// Confirmed against a real sample of `mapPlanets.jsonl` (68407
/// records, EVE Online, August 2026):
/// - `celestialIndex`, `position`, `typeID` and `solarSystemID` are
///   present in 100% of records -- unlike Python (which reads
///   `celestialIndex` with `.get()`, optional), here they're treated
///   as required ([`required_i64`]/[`required_position`]), same
///   criterion used throughout this file for `NOT NULL` columns
///   (`mapPlanets.planetaryIndex` is one) when the real source
///   confirms the data is always there: fail early with a clear
///   message instead of letting SQLite reject a `NULL` further down.
/// - `radius` is **always** at the top level (never needs the nested
///   fallback to `statistics.radius`) -- but unlike `mapStars.radius`
///   (an `INTEGER` column), `mapPlanets.radius` is `REAL`, so it's
///   read with [`optional_f64_with_nested_fallback`], not the `i64`
///   variant.
/// - `fragmented` **never** shows up, neither at the top level nor
///   nested (0 out of 68407) -- in practice this column always comes
///   out `NULL`.
/// - `locked`, on the other hand, is **always** nested under
///   `statistics` (never at the top level) -- the opposite of
///   `radius`. Here the fallback genuinely matters, to not lose the
///   data.
pub fn parse_planets(
    connection: &Connection,
    sde_directory: &Path,
    state: &SystemScopeState,
) -> Result<usize, BuilderError> {
    let mut insert_planet = connection.prepare(
        "INSERT INTO mapPlanets (planetId, solarSystemId, planetaryIndex, fragmented, radius, \
         locked, typeId, positionX, positionY, positionZ) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
    )?;

    let mut count = 0usize;
    for record in iter_jsonl_records(sde_directory, "mapPlanets")? {
        let record = record?;
        let solar_system_id = required_i64(&record, "solarSystemID")?;
        if !state.systems_in_scope.contains(&solar_system_id) {
            continue;
        }

        let id = required_i64(&record, "_key")?;
        let planet_index = required_i64(&record, "celestialIndex")?;
        let fragmented = optional_bool_with_nested_fallback(&record, "fragmented", "statistics");
        let radius = optional_f64_with_nested_fallback(&record, "radius", "statistics");
        let locked = optional_bool_with_nested_fallback(&record, "locked", "statistics");
        let type_id = required_i64(&record, "typeID")?;
        let (pos_x, pos_y, pos_z) = required_position(&record)?;

        insert_planet.execute(rusqlite::params![
            id,
            solar_system_id,
            planet_index,
            fragmented,
            radius,
            locked,
            type_id,
            pos_x,
            pos_y,
            pos_z,
        ])?;
        count += 1;
    }
    Ok(count)
}

// ---------------------------------------------------------------------
// mapMoons
// ---------------------------------------------------------------------

/// Populates `mapMoons` from `<sde_directory>/mapMoons.jsonl`, filtering
/// by `state.systems_in_scope` (populated by [`parse_solar_systems`]).
/// Requires `mapSolarSystems` to already be populated (FK). Returns the
/// number of rows inserted. Equivalent to `_parse_moons()` in Python.
///
/// # Note: no verification against real data
///
/// Confirmed against a real sample of `mapMoons.jsonl` (344457
/// records, EVE Online, August 2026): `celestialIndex`, `orbitID`,
/// `orbitIndex`, `typeID`, `position` and `solarSystemID` are present
/// in 100% of records -- matching the field list `_parse_moons()`'s
/// own docstring in Python already claimed for this entity (the one
/// that was originally used as the basis to *infer without
/// independently verifying* `mapStars`/`mapPlanets`'s shape in the two
/// previous phases -- that inference turned out correct). `locked` is
/// never at the top level, nested under `statistics` in 99.6% of
/// records -- but genuinely absent from both places in the remaining
/// 0.4% (1364 of 344457), confirming the nested fallback (see
/// [`optional_bool_with_nested_fallback`]) is exercised by real data,
/// not just a theoretical possibility.
///
/// `moonIndex` (`orbitIndex` in the JSON) is treated as required
/// ([`required_i64`]) -- confirmed present in every one of the 344457
/// real records checked, same criterion as `planetaryIndex` in the
/// previous phase.
///
/// `typeId` is also treated as required ([`required_i64`]), matching
/// Python's `moon['typeID']` (bracket) access and confirmed present in
/// every real record checked -- even though the column itself is
/// nullable in the schema (`typeId INTEGER REFERENCES
/// invTypes(typeId)`, without `NOT NULL`).
///
/// Real moon `position` magnitude checked too: up to ~1.8x10^13 in the
/// sample (about 0.2% of 2^53) -- far below the `i64 -> f64` precision
/// boundary discussed in [`crate::objects::MapPoint`]'s docstring.
/// Moon positions are system-scale (similar to `mapPlanets`'s
/// ~3x10^13), not galactic-scale like `mapRegions`/`mapSolarSystems`'s
/// ~10^19 -- no precision concern here, for this data or for any
/// future function that might expose it (`SdeManager::get_moon()`
/// doesn't read `position` today; `objects::Moon` has no coordinate
/// field to put it in).
pub fn parse_moons(
    connection: &Connection,
    sde_directory: &Path,
    state: &SystemScopeState,
) -> Result<usize, BuilderError> {
    let mut insert_moon = connection.prepare(
        "INSERT INTO mapMoons (moonId, solarSystemId, moonIndex, planetId, typeId, radius, \
         positionX, positionY, positionZ) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
    )?;

    let mut count = 0usize;
    for record in iter_jsonl_records(sde_directory, "mapMoons")? {
        let record = record?;
        let solar_system_id = required_i64(&record, "solarSystemID")?;
        if !state.systems_in_scope.contains(&solar_system_id) {
            continue;
        }

        let id = required_i64(&record, "_key")?;
        let moon_index = required_i64(&record, "orbitIndex")?;
        let planet_id = optional_i64(&record, "orbitID");
        let type_id = required_i64(&record, "typeID")?;
        let radius = optional_i64_with_nested_fallback(&record, "radius", "statistics");
        let (pos_x, pos_y, pos_z) = required_position(&record)?;

        insert_moon.execute(rusqlite::params![
            id,
            solar_system_id,
            moon_index,
            planet_id,
            type_id,
            radius,
            pos_x,
            pos_y,
            pos_z,
        ])?;
        count += 1;
    }
    Ok(count)
}

// ---------------------------------------------------------------------
// mapSystemConnections
// ---------------------------------------------------------------------

/// Populates `mapSystemConnections` from `mapSystemGates`, joining each
/// gate with the gate it points to (`destinationGateId`) to derive the
/// pair of solar systems it connects. Unlike every other function in
/// this file, this one does NOT read any SDE file -- the whole logic
/// is a single SQL statement over data already inserted by
/// [`parse_stargates`], which is why it doesn't take `sde_directory`.
/// Returns the number of rows inserted. Equivalent to
/// `parse_connections()` in Python (yes, no leading underscore -- it's
/// the only public `_parse_*`/`parse_*` in the prototype).
///
/// Requires `mapSystemGates` to already be populated. If
/// `config.with_gates` was `false` (so [`parse_stargates`] never ran)
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
/// `(msga.solarSystemId, msgb.solarSystemId)` in that order in
/// practice, but they're ported literally as they are in Python.
pub fn parse_connections(connection: &Connection) -> Result<usize, BuilderError> {
    let count = connection.execute(
        "INSERT INTO mapSystemConnections (systemA, systemB) \
         SELECT MIN(msga.solarSystemId, msgb.solarSystemId), \
                MAX(msga.solarSystemId, msgb.solarSystemId) \
         FROM mapSystemGates AS msga \
         INNER JOIN mapSystemGates AS msgb ON (msgb.systemGateId = msga.destinationGateId) \
         WHERE msga.solarSystemId < msgb.solarSystemId",
        [],
    )?;
    Ok(count)
}

// ---------------------------------------------------------------------
// stationServices / stationOperations / npcStations (phase 10)
// ---------------------------------------------------------------------

/// Populates `stationServices` from `<sde_directory>/stationServices.jsonl`
/// (27 records, confirmed complete: `_key`/`serviceName` present in
/// 100% of records). No equivalent in the Python prototype -- this
/// entity, along with `stationOperations`/`npcStations` below, was
/// added directly against the real SDE export, not ported from
/// `sde_parser.py` (see [`parse_npc_stations`]'s docstring for why
/// `staStation`/`staCorporations`, which *were* in both the schema and
/// the Python prototype, are gone).
pub fn parse_station_services(
    connection: &Connection,
    sde_directory: &Path,
    config: &ParserConfig,
) -> Result<usize, BuilderError> {
    let mut insert = connection
        .prepare("INSERT INTO stationServices (serviceId, serviceName) VALUES (?1, ?2)")?;

    let mut count = 0usize;
    for record in iter_jsonl_records(sde_directory, "stationServices")? {
        let record = record?;
        let id = required_i64(&record, "_key")?;
        let name = required_localized(&record, "serviceName", config)?;
        insert.execute(rusqlite::params![id, name])?;
        count += 1;
    }
    Ok(count)
}

/// Populates `stationOperations`, `stationOperationServices`, and
/// `stationOperationTypes` from
/// `<sde_directory>/stationOperations.jsonl` (68 records). Requires
/// [`parse_station_services`]/[`crate::builder::parser::parse_types`]
/// (phase 1, for `invTypes`) to have already run -- the two junction
/// tables reference `stationServices`/`invTypes`.
///
/// Confirmed against the real 68 records: `_key`, `activityID`,
/// `border`, `corridor`, `fringe`, `hub`, `manufacturingFactor`,
/// `operationName`, `ratio`, `researchFactor`, `services` are present
/// in 100% of records -- treated as required
/// ([`required_i64`]/[`required_f64`]/[`required_localized`]).
/// `description` is present in 55/68 (80.9%) -- optional
/// ([`localized`], not [`required_localized`]). `stationTypes` is
/// present in 47/68 (69.1%) -- also optional, only inserted into
/// `stationOperationTypes` when the record actually carries it.
///
/// Each `stationTypes` entry is `{"_key": <sizeKey>, "_value": <typeId>}`
/// -- `_key` takes one of exactly 5 values across all 68 records (1, 2,
/// 4, 8, 16, confirmed by exhaustive check), consistent with a
/// station-size bit-flag, though the SDE itself doesn't document what
/// each flag means beyond the raw value; `stationOperationTypes.sizeKey`
/// is kept as a plain integer rather than guessing at named constants.
pub fn parse_station_operations(
    connection: &Connection,
    sde_directory: &Path,
    config: &ParserConfig,
) -> Result<usize, BuilderError> {
    let mut insert_operation = connection.prepare(
        "INSERT INTO stationOperations \
         (operationId, activityId, operationName, description, border, corridor, fringe, hub, \
          ratio, manufacturingFactor, researchFactor) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
    )?;
    let mut insert_service = connection
        .prepare("INSERT INTO stationOperationServices (operationId, serviceId) VALUES (?1, ?2)")?;
    let mut insert_type = connection.prepare(
        "INSERT INTO stationOperationTypes (operationId, sizeKey, typeId) VALUES (?1, ?2, ?3)",
    )?;

    let mut count = 0usize;
    for record in iter_jsonl_records(sde_directory, "stationOperations")? {
        let record = record?;
        let id = required_i64(&record, "_key")?;
        let activity_id = required_i64(&record, "activityID")?;
        let name = required_localized(&record, "operationName", config)?;
        let description = localized(&record, "description", config);
        let border = required_f64(&record, "border")?;
        let corridor = required_f64(&record, "corridor")?;
        let fringe = required_f64(&record, "fringe")?;
        let hub = required_f64(&record, "hub")?;
        let ratio = required_f64(&record, "ratio")?;
        let manufacturing_factor = required_f64(&record, "manufacturingFactor")?;
        let research_factor = required_f64(&record, "researchFactor")?;

        insert_operation.execute(rusqlite::params![
            id,
            activity_id,
            name,
            description,
            border,
            corridor,
            fringe,
            hub,
            ratio,
            manufacturing_factor,
            research_factor
        ])?;

        for service_id in optional_i64_array(&record, "services")? {
            insert_service.execute(rusqlite::params![id, service_id])?;
        }

        if let Some(Value::Array(station_types)) = record.get("stationTypes") {
            for entry in station_types {
                let size_key = required_i64(entry, "_key")?;
                let type_id = required_i64(entry, "_value")?;
                insert_type.execute(rusqlite::params![id, size_key, type_id])?;
            }
        }

        count += 1;
    }
    Ok(count)
}

/// Populates `npcStations` from `<sde_directory>/npcStations.jsonl`
/// (5210 records). Requires `mapMoons`/`mapPlanets` (phases 7/8),
/// `mapSolarSystems` (phase 4), `npcCorporations` (phase 2), `invTypes`
/// (phase 1), and [`parse_station_operations`] to have already run --
/// every foreign key on this table points somewhere.
///
/// # Why this exists instead of `staStation`/`staCorporations`
///
/// Neither `staStation` nor `staCorporations` was ever populated, by
/// this port or by the original Python prototype (`_parse_station()`
/// never existed in `sde_parser.py`) -- a schema/parser mismatch
/// inherited from the reference implementation, confirmed by grepping
/// its source directly, not a gap introduced during this migration.
/// The real SDE export uses a different table name (`npcStations`, not
/// `staStation`) and a materially richer shape (reprocessing data,
/// station operation/services, precise real-world position), so this
/// isn't a rename of the old design -- it's built fresh against the
/// real data, and `staStation`/`staCorporations` are removed from the
/// schema entirely rather than left declared-but-dead.
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
/// moon) are both treated as optional ([`optional_i64`]), matching
/// their real, confirmed absence rate -- not just a defensive
/// assumption.
pub fn parse_npc_stations(
    connection: &Connection,
    sde_directory: &Path,
) -> Result<usize, BuilderError> {
    let mut moon_ids: std::collections::HashSet<i64> = std::collections::HashSet::new();
    {
        let mut statement = connection.prepare("SELECT moonId FROM mapMoons")?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            moon_ids.insert(row.get(0)?);
        }
    }
    let mut planet_ids: std::collections::HashSet<i64> = std::collections::HashSet::new();
    {
        let mut statement = connection.prepare("SELECT planetId FROM mapPlanets")?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            planet_ids.insert(row.get(0)?);
        }
    }

    let mut insert = connection.prepare(
        "INSERT INTO npcStations \
         (stationId, celestialIndex, operationId, orbitMoonId, orbitPlanetId, orbitIndex, \
          ownerId, positionX, positionY, positionZ, reprocessingEfficiency, \
          reprocessingHangarFlag, reprocessingStationsTake, solarSystemId, typeId, \
          useOperationName) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
    )?;

    let mut count = 0usize;
    for record in iter_jsonl_records(sde_directory, "npcStations")? {
        let record = record?;
        let id = required_i64(&record, "_key")?;
        let celestial_index = optional_i64(&record, "celestialIndex");
        let operation_id = required_i64(&record, "operationID")?;
        let orbit_id = required_i64(&record, "orbitID")?;
        let (orbit_moon_id, orbit_planet_id) = if moon_ids.contains(&orbit_id) {
            (Some(orbit_id), None)
        } else if planet_ids.contains(&orbit_id) {
            (None, Some(orbit_id))
        } else {
            (None, None)
        };
        let orbit_index = optional_i64(&record, "orbitIndex");
        let owner_id = required_i64(&record, "ownerID")?;
        let (x, y, z) = required_position(&record)?;
        let reprocessing_efficiency = required_f64(&record, "reprocessingEfficiency")?;
        let reprocessing_hangar_flag = required_i64(&record, "reprocessingHangarFlag")?;
        let reprocessing_stations_take = required_f64(&record, "reprocessingStationsTake")?;
        let solar_system_id = required_i64(&record, "solarSystemID")?;
        let type_id = required_i64(&record, "typeID")?;
        let use_operation_name = required_bool(&record, "useOperationName")?;

        insert.execute(rusqlite::params![
            id,
            celestial_index,
            operation_id,
            orbit_moon_id,
            orbit_planet_id,
            orbit_index,
            owner_id,
            x,
            y,
            z,
            reprocessing_efficiency,
            reprocessing_hangar_flag,
            reprocessing_stations_take,
            solar_system_id,
            type_id,
            use_operation_name
        ])?;
        count += 1;
    }
    Ok(count)
}

// ---------------------------------------------------------------------
// Orquestador
// ---------------------------------------------------------------------

/// Number of rows inserted by each phase of [`parse_data`].
///
/// `star_types` counts `typeStar`'s rows (not its own phase: they're
/// generated by [`parse_types`] when it detects "Sun"-group types).
/// `station_operation_services`/`station_operation_types` count rows
/// in those two junction tables (not their own phase either: they're
/// generated by [`parse_station_operations`]).
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

/// Runs the full parsing pipeline over `sde_directory`, in the same
/// dependency order as `parse_data()` in Python. Equivalent to that
/// method, except for the current scope (see below).
///
/// Unlike the individual `parse_*` functions -- which autocommit each
/// `INSERT` separately, see "Transactions" in the module's docstring --
/// this function DOES wrap the whole pipeline in a single explicit
/// transaction (`Connection::transaction()`), same as Python, which
/// doesn't `commit()` until `SdeParser.close()`, at the very end. If
/// any phase fails, EVERYTHING inserted up to that point gets rolled
/// back -- nothing is left half-persisted -- because rusqlite's
/// `Transaction` rolls back automatically on `Drop` if `.commit()` was
/// never called, and each call below's `?` operator triggers exactly
/// that early `Drop` when it propagates the error.
///
/// Requires `&mut Connection` (not `&Connection` like the individual
/// functions) because `Connection::transaction()` requires it.
///
/// ## Current scope
///
/// Covers the 14 functions ported from Python (phase 1 to phase 9):
/// categories, groups, types (+ `typeStar`), races, NPC corporations,
/// factions (+ `factionRace`), regions, constellations, solar systems,
/// stargates (gated by `config.with_gates`), stars, planets, moons
/// (gated by `config.with_moons`) and connections -- **full parity**
/// with Python's `parse_data()`. Phase 10 (`stationServices`,
/// `stationOperations` + its two junction tables, `npcStations`) has
/// no Python equivalent -- see [`parse_npc_stations`]'s docstring for
/// why. It runs last and unconditionally (no config flag gates it, same
/// as most phases besides gates/moons), but its `orbitMoonId`
/// resolution depends on `parse_moons`/`parse_planets` having already
/// populated `mapMoons`/`mapPlanets` -- if `config.with_moons` was
/// `false`, every station that would otherwise resolve to a moon
/// resolves to neither instead (both `orbitMoonId`/`orbitPlanetId`
/// `NULL`), same as the one genuinely-neither station in the real data.
pub fn parse_data(
    connection: &mut Connection,
    sde_directory: &Path,
    config: &ParserConfig,
) -> Result<ParseSummary, BuilderError> {
    let tx = connection.transaction()?;

    let categories = parse_categories(&tx, sde_directory, config)?;
    let mut state = StarTypeState::default();
    let groups = parse_groups(&tx, sde_directory, config, &mut state)?;
    let types = parse_types(&tx, sde_directory, config, &mut state)?;
    let races = parse_races(&tx, sde_directory, config)?;
    let npc_corporation_divisions = parse_npc_corporation_divisions(&tx, sde_directory, config)?;
    let npc_corporations = parse_npc_corporations(&tx, sde_directory, config)?;
    let factions = parse_factions(&tx, sde_directory, config)?;
    let regions = parse_regions(&tx, sde_directory, config)?;
    let constellations = parse_constellations(&tx, sde_directory, config)?;
    let mut scope = SystemScopeState::default();
    let solar_systems = parse_solar_systems(&tx, sde_directory, config, &mut scope)?;
    let stargates = if config.with_gates {
        parse_stargates(&tx, sde_directory, &scope)?
    } else {
        0
    };
    let stars = parse_stars(&tx, sde_directory, &scope, &state)?;
    let planets = parse_planets(&tx, sde_directory, &scope)?;
    let moons = if config.with_moons {
        parse_moons(&tx, sde_directory, &scope)?
    } else {
        0
    };
    let connections = parse_connections(&tx)?;

    let station_services = parse_station_services(&tx, sde_directory, config)?;
    let station_operations = parse_station_operations(&tx, sde_directory, config)?;
    let station_operation_services: usize =
        tx.query_row("SELECT COUNT(*) FROM stationOperationServices", [], |row| {
            row.get::<usize, i64>(0)
        })? as usize;
    let station_operation_types: usize =
        tx.query_row("SELECT COUNT(*) FROM stationOperationTypes", [], |row| {
            row.get::<usize, i64>(0)
        })? as usize;
    let npc_stations = parse_npc_stations(&tx, sde_directory)?;

    // Diagnostic: PRAGMA foreign_key_check runs within this transaction,
    // before COMMIT, so it can point at exactly which row/table/FK is
    // unsatisfied -- instead of letting a bare `tx.commit()` fail with
    // SQLite's generic "FOREIGN KEY constraint failed" (no indication of
    // which of this crate's several DEFERRABLE constraints -- across
    // npcCorporations/npcStations/factions -- is the actual culprit).
    // Real EVE data is large enough (thousands of NPC corporations) that
    // guessing at the cause from the generic message alone isn't
    // reliable; this turns a silent COMMIT failure into a precise,
    // actionable one. foreign_key_check only gives a numeric fk index
    // (not a column name), so foreign_key_list(<table>) is queried too
    // (cached per table, since multiple violations often share one) to
    // translate that index into the actual column.
    {
        let mut fk_list_cache: std::collections::HashMap<
            String,
            std::collections::HashMap<i64, String>,
        > = std::collections::HashMap::new();
        let mut check = tx.prepare("PRAGMA foreign_key_check")?;
        let mut rows = check.query([])?;
        let mut violations = Vec::new();
        while let Some(row) = rows.next()? {
            let table: String = row.get(0)?;
            let rowid: Option<i64> = row.get(1)?;
            let parent: String = row.get(2)?;
            let fkid: i64 = row.get(3)?;

            if !fk_list_cache.contains_key(&table) {
                let mut column_by_fkid = std::collections::HashMap::new();
                let mut fk_list = tx.prepare(&format!("PRAGMA foreign_key_list({table})"))?;
                let mut fk_rows = fk_list.query([])?;
                while let Some(fk_row) = fk_rows.next()? {
                    let id: i64 = fk_row.get(0)?;
                    let from_column: String = fk_row.get(3)?;
                    column_by_fkid.insert(id, from_column);
                }
                fk_list_cache.insert(table.clone(), column_by_fkid);
            }
            let column = fk_list_cache
                .get(&table)
                .and_then(|m| m.get(&fkid))
                .map(String::as_str)
                .unwrap_or("<unknown column>");

            let rowid_str = rowid
                .map(|r| r.to_string())
                .unwrap_or_else(|| "N/A".to_string());
            violations.push(format!(
                "table {table}, rowid {rowid_str}, column {column} references {parent}"
            ));
        }
        if !violations.is_empty() {
            return Err(BuilderError::Data(format!(
                "foreign_key_check found {} unsatisfied constraint(s) before commit:\n  {}",
                violations.len(),
                violations.join("\n  ")
            )));
        }
    }

    tx.commit()?;

    Ok(ParseSummary {
        categories,
        groups,
        types,
        races,
        npc_corporation_divisions,
        npc_corporations,
        factions,
        star_types: state.star_type_ids.len(),
        regions,
        constellations,
        solar_systems,
        stargates,
        stars,
        planets,
        moons,
        connections,
        station_services,
        station_operations,
        station_operation_services,
        station_operation_types,
        npc_stations,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    /// Unique temp directory with the given `.jsonl` files (name ->
    /// content), removed automatically on going out of scope. Same
    /// pattern as `tests/manager.rs`'s fixture.
    struct TempSdeDir {
        path: std::path::PathBuf,
    }

    impl TempSdeDir {
        fn new(test_name: &str, files: &[(&str, &str)]) -> Self {
            let id = COUNTER.fetch_add(1, Ordering::SeqCst);
            let path = std::env::temp_dir().join(format!(
                "sde_parser_test_{}_{}_{}",
                test_name,
                std::process::id(),
                id
            ));
            std::fs::create_dir_all(&path).expect("cannot create temp sde dir");
            for (name, content) in files {
                std::fs::write(path.join(name), content).expect("cannot write fixture file");
            }
            Self { path }
        }
    }

    impl Drop for TempSdeDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

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

        let count = parse_categories(&connection, &dir.path, &config).unwrap();
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

        let count = parse_races(&connection, &dir.path, &config).unwrap();
        assert_eq!(count, 2);

        let name: String = connection
            .query_row("SELECT raceName FROM races WHERE raceId = 1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(name, "Caldari");
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
        let mut state = StarTypeState::default();

        let groups = parse_groups(&connection, &dir.path, &config, &mut state).unwrap();
        assert_eq!(groups, 2);
        assert_eq!(state.sun_group_id, Some(6));

        let types = parse_types(&connection, &dir.path, &config, &mut state).unwrap();
        assert_eq!(types, 2);

        // The "Sun"-group type should have generated a row in typeStar.
        assert_eq!(state.star_type_ids.len(), 1);
        let star_type_id = state.star_type_ids[&3000];
        let (name, color): (String, String) = connection
            .query_row(
                "SELECT name, color FROM typeStar WHERE starTypeId = ?1",
                rusqlite::params![star_type_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(name, "G5");
        assert_eq!(color, "ffcc00");

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

        let result = parse_categories(&connection, &dir.path, &config);
        assert!(result.is_err());
    }

    #[test]
    fn parse_categories_missing_name_errors() {
        // categoryName is TEXT NOT NULL in the STRICT schema; Python
        // would also fail here (IntegrityError inserting NULL) -- see
        // `required_localized`'s docstring.
        let dir = TempSdeDir::new("missing_name", &[("categories.jsonl", "{\"_key\": 6}\n")]);
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        let config = ParserConfig::default();

        let result = parse_categories(&connection, &dir.path, &config);
        assert!(result.is_err());
    }

    #[test]
    fn parse_categories_missing_file_errors() {
        let dir = TempSdeDir::new("missing_file", &[]);
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        let config = ParserConfig::default();

        // categories.jsonl was never written at all.
        let result = parse_categories(&connection, &dir.path, &config);
        assert!(result.is_err());
    }

    #[test]
    fn localized_falls_back_to_english() {
        let config = ParserConfig {
            language: "fr".to_string(),
            ..Default::default()
        };
        let record: Value =
            serde_json::from_str(r#"{"name": {"en": "Jita", "de": "Jita"}}"#).unwrap();
        // "fr" isn't present -> falls back to "en".
        assert_eq!(localized(&record, "name", &config), Some("Jita"));
    }

    #[test]
    fn localized_uses_requested_language_when_present() {
        let config = ParserConfig {
            language: "de".to_string(),
            ..Default::default()
        };
        let record: Value =
            serde_json::from_str(r#"{"name": {"en": "Jita", "de": "Jita (de)"}}"#).unwrap();
        assert_eq!(localized(&record, "name", &config), Some("Jita (de)"));
    }

    #[test]
    fn isometric_projection_2d_matches_python_reference_values() {
        // Reference values computed by running
        // calculate_isometric_projection() from sde_parser.py directly
        // with x=100.0, y=200.0, z=300.0 for each projected_axis
        // (0/1/2), taking the two non-zero-forced components from the
        // 3-tuple.
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
    fn parser_config_default_uses_y_axis_and_does_not_force_isometric() {
        // Matches SdeConfig's real defaults in Python:
        // projection_algorithm='isometric', projected_axis=1 (Y) -- but
        // here the "forcing" is off by default, since normal behavior
        // is to trust the position2D CCP already provides when it's
        // present.
        let config = ParserConfig::default();
        assert!(!config.force_isometric_position_2d);
        assert_eq!(config.isometric_projected_axis, ProjectedAxis::Y);
    }

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

        let count = parse_npc_corporations(&connection, &dir.path, &config).unwrap();
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

        let count = parse_npc_corporations(&connection, &dir.path, &config).unwrap();
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
        // tickerName is TEXT NOT NULL and is accessed as a required field
        // (equivalent to corporation['tickerName'] in Python).
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

        let result = parse_npc_corporations(&connection, &dir.path, &config);
        assert!(result.is_err());
    }

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

        let count = parse_factions(&connection, &dir.path, &config).unwrap();
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

        let count = parse_factions(&connection, &dir.path, &config).unwrap();
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

        let result = parse_factions(&connection, &dir.path, &config);
        assert!(result.is_err());
    }

    #[test]
    fn parse_factions_missing_size_factor_errors() {
        // sizeFactor is REAL NOT NULL and is accessed as a required field
        // (equivalent to faction['sizeFactor'] in Python).
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

        let result = parse_factions(&connection, &dir.path, &config);
        assert!(result.is_err());
    }

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

        let count = parse_regions(&connection, &dir.path, &config).unwrap();
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
        // mapRegions.nebula is INTEGER NOT NULL; Python reads it as
        // optional (`region.get('nebulaID')`) but would fail the same
        // way on insert if it were missing -- see "Known deviations" in
        // the module's docstring.
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

        let result = parse_regions(&connection, &dir.path, &config);
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

        let result = parse_regions(&connection, &dir.path, &config);
        assert!(result.is_err());
    }

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

        let count = parse_constellations(&connection, &dir.path, &config).unwrap();
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

        parse_constellations(&connection, &dir.path, &config).unwrap();

        let id: i64 = connection
            .query_row("SELECT constellationId FROM mapConstellations", [], |row| {
                row.get(0)
            })
            .unwrap();
        // constellationID (20000020) wins over _key (999).
        assert_eq!(id, 20000020);
    }

    #[test]
    fn system_in_scope_kspace_gates_on_map_kspace() {
        let mut config = ParserConfig::default();
        assert!(system_in_scope(None, &config)); // default: map_kspace=true
        config.map_kspace = false;
        assert!(!system_in_scope(None, &config));
    }

    #[test]
    fn system_in_scope_wormhole_gates_on_any_of_three_flags() {
        let mut config = ParserConfig {
            map_wspace: false,
            map_abyssal: false,
            map_void: false,
            ..Default::default()
        };
        assert!(!system_in_scope(Some(5), &config));
        config.map_wspace = true;
        assert!(system_in_scope(Some(5), &config));
    }

    #[test]
    fn parse_solar_systems_inserts_kspace_system_with_ccp_position2d() {
        let dir = TempSdeDir::new(
            "solar_systems_kspace",
            &[(
                "mapSolarSystems.jsonl",
                "{\"_key\": 30000142, \"name\": {\"en\": \"Jita\"}, \"constellationID\": 20000020, \
                 \"radius\": 999999999.0, \"position\": {\"x\": -100.0, \"y\": 200.0, \"z\": -300.0}, \
                 \"securityStatus\": 0.9459, \"securityClass\": \"B\", \"corridor\": false, \
                 \"fringe\": false, \"hub\": true, \"international\": true, \"regional\": true, \
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
        let config = ParserConfig::default();
        let mut scope = SystemScopeState::default();

        let count = parse_solar_systems(&connection, &dir.path, &config, &mut scope).unwrap();
        assert_eq!(count, 1);
        assert!(scope.systems_in_scope.contains(&30000142));

        let (name, security, security_class, p2dx, p2dy): (String, f64, String, f64, f64) =
            connection
                .query_row(
                    "SELECT solarSystemName, security, securityClass, \
                     position2DX, position2DY FROM mapSolarSystems WHERE solarSystemId = 30000142",
                    [],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
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
    }

    #[test]
    fn parse_solar_systems_force_isometric_ignores_ccp_position2d() {
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
            force_isometric_position_2d: true,
            ..Default::default()
        };
        let mut scope = SystemScopeState::default();

        parse_solar_systems(&connection, &dir.path, &config, &mut scope).unwrap();

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
        let mut scope = SystemScopeState::default();

        let count = parse_solar_systems(&connection, &dir.path, &config, &mut scope).unwrap();
        assert_eq!(count, 1);
        assert!(!scope.systems_in_scope.contains(&1));
        assert!(scope.systems_in_scope.contains(&2));

        let total: i64 = connection
            .query_row("SELECT COUNT(*) FROM mapSolarSystems", [], |row| row.get(0))
            .unwrap();
        assert_eq!(total, 1);
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
        let mut scope = SystemScopeState::default();

        let result = parse_solar_systems(&connection, &dir.path, &config, &mut scope);
        assert!(result.is_err());
    }

    /// FK prerequisites shared by `parse_stargates`'s tests: two solar
    /// systems (30000001, 30000002) in the same constellation, and the
    /// item type (16, "Stargate") referenced by `mapSystemGates.typeId`.
    fn insert_stargate_prerequisites(connection: &Connection) {
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
        for (id, name) in [(30000001, "A"), (30000002, "B")] {
            connection
                .execute(
                    "INSERT INTO mapSolarSystems \
                     (solarSystemId, solarSystemName, constellationId, radius, centerX, centerY, centerZ, security) \
                     VALUES (?1, ?2, 20000020, 1.0, 0, 0, 0, 0.5)",
                    rusqlite::params![id, name],
                )
                .unwrap();
        }
    }

    /// Fixture of two mutually-referencing stargates: gate 50000001 (in
    /// system 30000001) points to 50000002 (in 30000002), and vice
    /// versa -- the typical case in real SDE data.
    const MUTUAL_STARGATES_JSONL: &str = "{\"_key\": 50000001, \"solarSystemID\": 30000001, \"typeID\": 16, \
         \"position\": {\"x\": 1.0, \"y\": 2.0, \"z\": 3.0}, \
         \"destination\": {\"stargateID\": 50000002, \"solarSystemID\": 30000002}}\n\
         {\"_key\": 50000002, \"solarSystemID\": 30000002, \"typeID\": 16, \
         \"position\": {\"x\": 4.0, \"y\": 5.0, \"z\": 6.0}, \
         \"destination\": {\"stargateID\": 50000001, \"solarSystemID\": 30000001}}\n";

    #[test]
    fn parse_stargates_without_transaction_fails_on_mutual_reference() {
        // Documents the behavior described in parse_stargates's
        // docstring: without an explicit transaction, SQLite operates
        // in autocommit mode (each INSERT is its own implicit
        // transaction), so destinationGateId's DEFERRABLE FK still
        // gets validated immediately -- and the first gate of the pair
        // necessarily references one that doesn't exist yet.
        let dir = TempSdeDir::new(
            "stargates_no_tx",
            &[("mapStargates.jsonl", MUTUAL_STARGATES_JSONL)],
        );
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        insert_stargate_prerequisites(&connection);
        let mut scope = SystemScopeState::default();
        scope.systems_in_scope.insert(30000001);
        scope.systems_in_scope.insert(30000002);

        let result = parse_stargates(&connection, &dir.path, &scope);
        assert!(result.is_err());
    }

    #[test]
    fn parse_stargates_within_transaction_inserts_mutual_reference() {
        let dir = TempSdeDir::new(
            "stargates_tx",
            &[("mapStargates.jsonl", MUTUAL_STARGATES_JSONL)],
        );
        let mut connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        insert_stargate_prerequisites(&connection);
        let mut scope = SystemScopeState::default();
        scope.systems_in_scope.insert(30000001);
        scope.systems_in_scope.insert(30000002);

        let tx = connection.transaction().unwrap();
        let count = parse_stargates(&tx, &dir.path, &scope).unwrap();
        assert_eq!(count, 2);
        tx.commit().unwrap();

        let (dest_gate, dest_system): (i64, i64) = connection
            .query_row(
                "SELECT destinationGateId, destinationSystemId FROM mapSystemGates \
                 WHERE systemGateId = 50000001",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(dest_gate, 50000002);
        assert_eq!(dest_system, 30000002);
    }

    #[test]
    fn parse_stargates_skips_systems_outside_scope() {
        let dir = TempSdeDir::new(
            "stargates_scope",
            &[(
                "mapStargates.jsonl",
                "{\"_key\": 50000003, \"solarSystemID\": 30000003, \"typeID\": 16, \
                 \"position\": {\"x\": 0.0, \"y\": 0.0, \"z\": 0.0}, \
                 \"destination\": {\"stargateID\": 50000004, \"solarSystemID\": 30000001}}\n",
            )],
        );
        let mut connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        insert_stargate_prerequisites(&connection);
        // 30000003 is NOT in scope (unlike 30000001/30000002).
        let mut scope = SystemScopeState::default();
        scope.systems_in_scope.insert(30000001);
        scope.systems_in_scope.insert(30000002);

        let tx = connection.transaction().unwrap();
        let count = parse_stargates(&tx, &dir.path, &scope).unwrap();
        tx.commit().unwrap();
        assert_eq!(count, 0);

        let total: i64 = connection
            .query_row("SELECT COUNT(*) FROM mapSystemGates", [], |row| row.get(0))
            .unwrap();
        assert_eq!(total, 0);
    }

    #[test]
    fn parse_stargates_missing_type_id_errors() {
        let dir = TempSdeDir::new(
            "stargates_missing_type",
            &[(
                "mapStargates.jsonl",
                "{\"_key\": 50000001, \"solarSystemID\": 30000001, \
                 \"position\": {\"x\": 0.0, \"y\": 0.0, \"z\": 0.0}, \
                 \"destination\": {\"stargateID\": 50000002, \"solarSystemID\": 30000002}}\n",
            )],
        );
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        insert_stargate_prerequisites(&connection);
        let mut scope = SystemScopeState::default();
        scope.systems_in_scope.insert(30000001);

        let result = parse_stargates(&connection, &dir.path, &scope);
        assert!(result.is_err());
    }

    /// Common setup for `parse_stars`'s tests: creates the schema, a
    /// detected star type ("Sun" > "Yellow G5 (ffcc00)") via
    /// `parse_groups`/`parse_types` directly against dedicated fixtures
    /// (to get a real `StarTypeState`, not a hand-simulated one), and a
    /// solar system in scope. Returns `(connection, star_state, scope)`.
    fn setup_for_parse_stars(dir_prefix: &str) -> (Connection, StarTypeState, SystemScopeState) {
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();

        let types_dir = TempSdeDir::new(
            dir_prefix,
            &[
                (
                    "groups.jsonl",
                    "{\"_key\": 6, \"categoryID\": 6, \"name\": {\"en\": \"Sun\"}, \"anchorable\": false}\n",
                ),
                (
                    "types.jsonl",
                    "{\"_key\": 3000, \"groupID\": 6, \"name\": {\"en\": \"Yellow G5 (ffcc00)\"}, \
                     \"iconID\": 100, \"published\": true, \"volume\": 0.0}\n",
                ),
            ],
        );
        connection
            .execute(
                "INSERT INTO invCategories (categoryId, categoryName, published) \
                 VALUES (6, 'Celestial', 1)",
                [],
            )
            .unwrap();
        let config = ParserConfig::default();
        let mut star_state = StarTypeState::default();
        parse_groups(&connection, &types_dir.path, &config, &mut star_state).unwrap();
        parse_types(&connection, &types_dir.path, &config, &mut star_state).unwrap();

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
                "INSERT INTO mapSolarSystems \
                 (solarSystemId, solarSystemName, constellationId, radius, centerX, centerY, centerZ, security) \
                 VALUES (30000001, 'A', 20000020, 1.0, 0, 0, 0, 0.5)",
                [],
            )
            .unwrap();

        let mut scope = SystemScopeState::default();
        scope.systems_in_scope.insert(30000001);

        (connection, star_state, scope)
    }

    #[test]
    fn parse_stars_inserts_row_using_real_sde_shape() {
        // Record with the shape confirmed against a real sample of
        // mapStars.jsonl (August 2026): radius as a top-level integer,
        // statistics present, locked nowhere to be found.
        let dir = TempSdeDir::new(
            "stars_real_shape",
            &[(
                "mapStars.jsonl",
                "{\"_key\": 40000001, \"radius\": 63350000, \"solarSystemID\": 30000001, \
                 \"statistics\": {\"age\": 4.5e17, \"life\": 6.9e17, \"luminosity\": 0.01575, \
                 \"spectralClass\": \"K2 V\", \"temperature\": 4567.0}, \"typeID\": 3000}\n",
            )],
        );
        let (connection, star_state, scope) = setup_for_parse_stars("stars_setup_real");

        let count = parse_stars(&connection, &dir.path, &scope, &star_state).unwrap();
        assert_eq!(count, 1);

        let (solar_system_id, locked, radius): (i64, Option<i64>, i64) = connection
            .query_row(
                "SELECT solarSystemId, locked, radius FROM mapStars WHERE starId = 40000001",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(solar_system_id, 30000001);
        assert_eq!(locked, None);
        assert_eq!(radius, 63350000);
    }

    #[test]
    fn parse_stars_locked_falls_back_to_nested_statistics() {
        // Synthetic -- the real SDE never carries `locked` (neither at
        // the top level nor in `statistics`), but the fallback is
        // ported from Python all the same, in case some other SDE
        // version does carry it.
        let dir = TempSdeDir::new(
            "stars_locked_fallback",
            &[(
                "mapStars.jsonl",
                "{\"_key\": 40000001, \"solarSystemID\": 30000001, \"typeID\": 3000, \
                 \"statistics\": {\"locked\": true}}\n",
            )],
        );
        let (connection, star_state, scope) = setup_for_parse_stars("stars_setup_fallback");

        parse_stars(&connection, &dir.path, &scope, &star_state).unwrap();

        let locked: Option<i64> = connection
            .query_row(
                "SELECT locked FROM mapStars WHERE starId = 40000001",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(locked, Some(1));
    }

    #[test]
    fn parse_stars_skips_systems_outside_scope() {
        let dir = TempSdeDir::new(
            "stars_scope",
            &[(
                "mapStars.jsonl",
                "{\"_key\": 40000001, \"radius\": 1, \"solarSystemID\": 30000099, \"typeID\": 3000}\n",
            )],
        );
        let (connection, star_state, scope) = setup_for_parse_stars("stars_setup_scope");
        // 30000099 is not in scope (only 30000001 is).

        let count = parse_stars(&connection, &dir.path, &scope, &star_state).unwrap();
        assert_eq!(count, 0);

        let total: i64 = connection
            .query_row("SELECT COUNT(*) FROM mapStars", [], |row| row.get(0))
            .unwrap();
        assert_eq!(total, 0);
    }

    #[test]
    fn parse_stars_unknown_star_type_errors() {
        let dir = TempSdeDir::new(
            "stars_unknown_type",
            &[(
                "mapStars.jsonl",
                // typeID 9999 is never detected as a star type
                // by parse_types() in this fixture.
                "{\"_key\": 40000001, \"radius\": 1, \"solarSystemID\": 30000001, \"typeID\": 9999}\n",
            )],
        );
        let (connection, star_state, scope) = setup_for_parse_stars("stars_setup_unknown");

        let result = parse_stars(&connection, &dir.path, &scope, &star_state);
        assert!(result.is_err());
    }

    /// Common setup for `parse_planets`'s tests: schema, a minimal
    /// `invTypes` to satisfy `typeId`'s FK, and a solar system in
    /// scope. Returns `(connection, scope)`.
    fn setup_for_parse_planets() -> (Connection, SystemScopeState) {
        let connection = Connection::open_in_memory().unwrap();
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
                 VALUES (1, 1, 'Planet', 0)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO invTypes (typeId, groupId, typeName, published) \
                 VALUES (11, 1, 'Planet (Barren)', 1)",
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
        connection
            .execute(
                "INSERT INTO mapSolarSystems \
                 (solarSystemId, solarSystemName, constellationId, radius, centerX, centerY, centerZ, security) \
                 VALUES (30000001, 'A', 20000020, 1.0, 0, 0, 0, 0.5)",
                [],
            )
            .unwrap();

        let mut scope = SystemScopeState::default();
        scope.systems_in_scope.insert(30000001);
        (connection, scope)
    }

    #[test]
    fn parse_planets_inserts_row_using_real_sde_shape() {
        // Real mapPlanets.jsonl record (August 2026, EVE Online):
        // celestialIndex/position/typeID/solarSystemID always present;
        // radius at the top level; locked ALWAYS nested under
        // statistics (never at the top level); fragmented absent.
        let dir = TempSdeDir::new(
            "planets_real_shape",
            &[(
                "mapPlanets.jsonl",
                "{\"_key\": 40000002, \"celestialIndex\": 1, \
                 \"position\": {\"x\": 161891117336.0, \"y\": 21288951986.0, \"z\": -73529712226.0}, \
                 \"radius\": 5060000, \"solarSystemID\": 30000001, \
                 \"statistics\": {\"locked\": false}, \"typeID\": 11}\n",
            )],
        );
        let (connection, scope) = setup_for_parse_planets();

        let count = parse_planets(&connection, &dir.path, &scope).unwrap();
        assert_eq!(count, 1);

        let (planetary_index, fragmented, radius, locked, type_id): (
            i64,
            Option<i64>,
            f64,
            i64,
            i64,
        ) = connection
            .query_row(
                "SELECT planetaryIndex, fragmented, radius, locked, typeId \
                 FROM mapPlanets WHERE planetId = 40000002",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(planetary_index, 1);
        assert_eq!(fragmented, None);
        assert_eq!(radius, 5060000.0);
        assert_eq!(locked, 0);
        assert_eq!(type_id, 11);
    }

    #[test]
    fn parse_planets_skips_systems_outside_scope() {
        let dir = TempSdeDir::new(
            "planets_scope",
            &[(
                "mapPlanets.jsonl",
                "{\"_key\": 40000002, \"celestialIndex\": 1, \
                 \"position\": {\"x\": 0.0, \"y\": 0.0, \"z\": 0.0}, \
                 \"radius\": 1, \"solarSystemID\": 30000099, \"typeID\": 11}\n",
            )],
        );
        let (connection, scope) = setup_for_parse_planets();
        // 30000099 is not in scope (only 30000001 is).

        let count = parse_planets(&connection, &dir.path, &scope).unwrap();
        assert_eq!(count, 0);

        let total: i64 = connection
            .query_row("SELECT COUNT(*) FROM mapPlanets", [], |row| row.get(0))
            .unwrap();
        assert_eq!(total, 0);
    }

    #[test]
    fn parse_planets_missing_celestial_index_errors() {
        let dir = TempSdeDir::new(
            "planets_missing_index",
            &[(
                "mapPlanets.jsonl",
                "{\"_key\": 40000002, \"position\": {\"x\": 0.0, \"y\": 0.0, \"z\": 0.0}, \
                 \"radius\": 1, \"solarSystemID\": 30000001, \"typeID\": 11}\n",
            )],
        );
        let (connection, scope) = setup_for_parse_planets();

        let result = parse_planets(&connection, &dir.path, &scope);
        assert!(result.is_err());
    }

    #[test]
    fn parse_planets_missing_position_errors() {
        let dir = TempSdeDir::new(
            "planets_missing_position",
            &[(
                "mapPlanets.jsonl",
                "{\"_key\": 40000002, \"celestialIndex\": 1, \
                 \"radius\": 1, \"solarSystemID\": 30000001, \"typeID\": 11}\n",
            )],
        );
        let (connection, scope) = setup_for_parse_planets();

        let result = parse_planets(&connection, &dir.path, &scope);
        assert!(result.is_err());
    }

    /// Common setup for `parse_moons`'s tests: schema, an `invTypes`
    /// row for the planet and another for the moon, a solar system and
    /// a planet in scope (so `planetId` can be tested with a real value
    /// as well as `NULL`). Returns `(connection, scope)`.
    fn setup_for_parse_moons() -> (Connection, SystemScopeState) {
        let connection = Connection::open_in_memory().unwrap();
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
                 VALUES (1, 1, 'Celestial Group', 0)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO invTypes (typeId, groupId, typeName, published) \
                 VALUES (11, 1, 'Planet (Barren)', 1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO invTypes (typeId, groupId, typeName, published) \
                 VALUES (12, 1, 'Moon', 1)",
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
                 VALUES (40000002, 30000001, 1, 11, 0, 0, 0)",
                [],
            )
            .unwrap();

        let mut scope = SystemScopeState::default();
        scope.systems_in_scope.insert(30000001);
        (connection, scope)
    }

    #[test]
    fn parse_moons_inserts_row_with_planet_reference() {
        let dir = TempSdeDir::new(
            "moons_with_planet",
            &[(
                "mapMoons.jsonl",
                "{\"_key\": 40000004, \"solarSystemID\": 30000001, \"orbitIndex\": 1, \
                 \"orbitID\": 40000002, \"typeID\": 12, \"radius\": 100000, \
                 \"position\": {\"x\": 1.0, \"y\": 2.0, \"z\": 3.0}}\n",
            )],
        );
        let (connection, scope) = setup_for_parse_moons();

        let count = parse_moons(&connection, &dir.path, &scope).unwrap();
        assert_eq!(count, 1);

        let (moon_index, planet_id, type_id, radius): (i64, Option<i64>, i64, Option<i64>) =
            connection
                .query_row(
                    "SELECT moonIndex, planetId, typeId, radius FROM mapMoons WHERE moonId = 40000004",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .unwrap();
        assert_eq!(moon_index, 1);
        assert_eq!(planet_id, Some(40000002));
        assert_eq!(type_id, 12);
        assert_eq!(radius, Some(100000));
    }

    #[test]
    fn parse_moons_without_orbit_id_leaves_planet_id_null() {
        // orbitID (planetId) is optional -- both in Python
        // (`moon.get('orbitID')`) and in the schema (a nullable column).
        let dir = TempSdeDir::new(
            "moons_no_planet",
            &[(
                "mapMoons.jsonl",
                "{\"_key\": 40000005, \"solarSystemID\": 30000001, \"orbitIndex\": 2, \
                 \"typeID\": 12, \"position\": {\"x\": 0.0, \"y\": 0.0, \"z\": 0.0}}\n",
            )],
        );
        let (connection, scope) = setup_for_parse_moons();

        parse_moons(&connection, &dir.path, &scope).unwrap();

        let planet_id: Option<i64> = connection
            .query_row(
                "SELECT planetId FROM mapMoons WHERE moonId = 40000005",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(planet_id, None);
    }

    #[test]
    fn parse_moons_skips_systems_outside_scope() {
        let dir = TempSdeDir::new(
            "moons_scope",
            &[(
                "mapMoons.jsonl",
                "{\"_key\": 40000004, \"solarSystemID\": 30000099, \"orbitIndex\": 1, \
                 \"typeID\": 12, \"position\": {\"x\": 0.0, \"y\": 0.0, \"z\": 0.0}}\n",
            )],
        );
        let (connection, scope) = setup_for_parse_moons();
        // 30000099 is not in scope (only 30000001 is).

        let count = parse_moons(&connection, &dir.path, &scope).unwrap();
        assert_eq!(count, 0);

        let total: i64 = connection
            .query_row("SELECT COUNT(*) FROM mapMoons", [], |row| row.get(0))
            .unwrap();
        assert_eq!(total, 0);
    }

    #[test]
    fn parse_moons_missing_orbit_index_errors() {
        let dir = TempSdeDir::new(
            "moons_missing_index",
            &[(
                "mapMoons.jsonl",
                "{\"_key\": 40000004, \"solarSystemID\": 30000001, \"typeID\": 12, \
                 \"position\": {\"x\": 0.0, \"y\": 0.0, \"z\": 0.0}}\n",
            )],
        );
        let (connection, scope) = setup_for_parse_moons();

        let result = parse_moons(&connection, &dir.path, &scope);
        assert!(result.is_err());
    }

    #[test]
    fn parse_moons_missing_type_id_errors() {
        let dir = TempSdeDir::new(
            "moons_missing_type",
            &[(
                "mapMoons.jsonl",
                "{\"_key\": 40000004, \"solarSystemID\": 30000001, \"orbitIndex\": 1, \
                 \"position\": {\"x\": 0.0, \"y\": 0.0, \"z\": 0.0}}\n",
            )],
        );
        let (connection, scope) = setup_for_parse_moons();

        let result = parse_moons(&connection, &dir.path, &scope);
        assert!(result.is_err());
    }

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

        let count = parse_connections(&connection).unwrap();
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

        let count = parse_connections(&connection).unwrap();
        assert_eq!(count, 0);

        let total: i64 = connection
            .query_row("SELECT COUNT(*) FROM mapSystemConnections", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(total, 0);
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

        let summary = parse_data(&mut connection, &dir.path, &config).unwrap();
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

        let error = parse_data(&mut connection, &dir.path, &config).unwrap_err();
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

        let result = parse_data(&mut connection, &dir.path, &config);
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

        let count = parse_station_services(&connection, &dir.path, &config).unwrap();
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

        let count = parse_station_operations(&connection, &dir.path, &config).unwrap();
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

        let count = parse_station_operations(&connection, &dir.path, &config).unwrap();
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

        let count = parse_npc_stations(&connection, &dir.path).unwrap();
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

        let count = parse_npc_stations(&connection, &dir.path).unwrap();
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

        let count = parse_npc_stations(&connection, &dir.path).unwrap();
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
