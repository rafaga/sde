//! Community data from dotlan: tables/columns that are NOT part of
//! CCP's official SDE, but come from external sources instead (dotlan's
//! SVG maps, and lists maintained by hand by the community, like Jove
//! Observatory systems or Triglavian invasion status).
//!
//! Unlike `builder::schema` (the static `STRICT` DDL that reconstructs
//! the canonical SDE), this module creates its tables/columns at
//! **runtime** (`CREATE TABLE`/`ALTER TABLE`), gated by
//! [`DotlanConfig`] -- an explicit decision: folding this into the
//! static schema would go beyond the crate's primary goal (reconstruct
//! the SDE as-is, not enrich it). A database built with, say,
//! `with_icebelts: false` simply doesn't have the `iceBelt` column at
//! all -- it's not that it exists empty.
//!
//! `mapAbstractSystems` is the only unconditional exception (same as in
//! Python, where `create_abstract_map()` always runs, no flag): it's
//! consumed by `SdeManager::get_abstract_systems()`/`get_abstract_connections()`
//! on the read side, so if this module (or [`update_tables`]) never ran
//! against the database, those two queries will fail with "no such
//! table".
//!
//! Equivalent to `ExternalParser`/`ExternalConfig` in the Python
//! prototype (`external_parser.py`).
//!
//! ## Current scope
//!
//! Covers `update_tables()` in full: [`create_abstract_map`],
//! [`create_icebelts`], [`setup_triglavian_status`],
//! [`setup_jove_observatories`], [`setup_special_anomalies`], and now
//! also [`extract_map_data`] (parsing an already-downloaded SVG,
//! equivalent to `_extract_map_data()` in Python). Missing: the
//! orchestrator that downloads maps region by region and retries on
//! error (`process()`), which will reuse [`super::http`]/
//! [`super::manifest`], already ported -- left for a later phase.

use crate::builder::manifest::{self, Manifest};
use crate::builder::{BuilderError, http};
use reqwest::Client;
use rusqlite::Connection;
use std::path::Path;

/// SVG XML namespace -- dotlan's maps use
/// `{http://www.w3.org/2000/svg}rect`/`use` in their original Python
/// XPath; `roxmltree` expresses the same thing via the tuple form
/// `(namespace, local_name)` of [`roxmltree::Node::has_tag_name`].
const SVG_NS: &str = "http://www.w3.org/2000/svg";

/// List of systems with a Jove Observatory, one per line. Extracted
/// programmatically (not hand-transcribed) from the three lists
/// concatenated in the original Python's `create_jove_observatories()`
/// -- 1032 names in the source, with 3 exact duplicates (`Eygfe`,
/// `MJYW-3`, `Odinesyn`) that were already duplicated there too; here
/// they're deduplicated (1029 unique names, same practical result: an
/// `UPDATE ... WHERE x IN (...)` doesn't change behavior because of a
/// repeated entry, so there's no loss of fidelity, just less redundant
/// text).
const JOVE_OBSERVATORY_SYSTEMS: &str = include_str!("jove_observatories.txt");

/// Config for dotlan's community data -- equivalent to `ExternalConfig`
/// in Python, same defaults (including `with_jove_observatories: true`,
/// the only flag among the four that starts out `true`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DotlanConfig {
    pub with_icebelts: bool,
    pub with_triglavian_status: bool,
    pub with_jove_observatories: bool,
    pub with_special_ore: bool,
}

impl Default for DotlanConfig {
    fn default() -> Self {
        Self {
            with_icebelts: false,
            with_triglavian_status: false,
            with_jove_observatories: true,
            with_special_ore: false,
        }
    }
}

/// Creates `mapAbstractSystems`, the table where dotlan's SVG parsing
/// (still to be ported) inserts the "abstract" 2D coordinates dotlan
/// computes for its own map layout -- unrelated to
/// `mapSolarSystems.position2DX/Y`, which come from the official SDE.
/// Always created, without gating on any [`DotlanConfig`] flag -- same
/// behavior as `create_abstract_map()` in Python.
///
/// `x`/`y` are `REAL`, not `INTEGER` as in Python's original DDL:
/// Python's original table isn't `STRICT` (classic SQLite accepts a
/// fractional value in an "INT" column anyway, via type affinity), but
/// this one is -- and the coordinates of a `<use x="..." y="...">` SVG
/// element are almost certainly fractional. `REAL` also already
/// matches what `SdeManager::get_abstract_systems()` expects on the
/// read side (`row.get::<usize, f32>(...)`), and what `tests/manager.rs`'s
/// fixture already uses.
pub fn create_abstract_map(connection: &Connection) -> Result<(), BuilderError> {
    connection.execute_batch(
        "CREATE TABLE mapAbstractSystems ( \
            solarSystemId INTEGER NOT NULL \
                REFERENCES mapSolarSystems(solarSystemId) \
                ON UPDATE CASCADE ON DELETE SET NULL, \
            regionId INTEGER NOT NULL \
                REFERENCES mapRegions(regionId) \
                ON UPDATE CASCADE ON DELETE SET NULL, \
            x REAL NOT NULL, y REAL NOT NULL, \
            CONSTRAINT pkey PRIMARY KEY (solarSystemId, regionId) ON CONFLICT FAIL \
        ) STRICT;",
    )?;
    Ok(())
}

/// Adds the `iceBelt` column (and its index) to `mapSolarSystems` --
/// **structure only**, doesn't populate it. Actual population depends
/// on parsing each regional SVG (`<rect class="i" id="...">`), so it
/// lives alongside that parsing (a later phase), not here. Equivalent
/// to `create_icebelts()` in Python.
pub fn create_icebelts(connection: &Connection) -> Result<(), BuilderError> {
    connection.execute_batch(
        "ALTER TABLE mapSolarSystems ADD COLUMN iceBelt \
            INTEGER NOT NULL DEFAULT 0 CHECK (iceBelt IN (0,1)); \
         CREATE INDEX icebelts ON mapSolarSystems (solarSystemId, iceBelt);",
    )?;
    Ok(())
}

/// Creates `mapTriglavianStatus` (with its 5 fixed rows), adds
/// `mapSolarSystems.trigStatusID`, and marks the systems for each of
/// the 4 non-`None` statuses -- structure and data together, in a
/// single function, same as `create_triglavian()` in Python (which
/// also doesn't separate the two: the triglavian data is 192 fixed ids
/// written directly in the code, not something derived from a separate
/// external source like the SVG).
///
/// # Necessary deviation from Python: no `DEFAULT 0` on the column
///
/// Python declares the column as
/// `trigStatusID INTEGER DEFAULT 0 REFERENCES mapTriglavianStatus(...)`.
/// Verified against real sqlite3: that combination -- a non-null
/// `DEFAULT` + `REFERENCES` -- **SQLite rejects it** in an `ALTER TABLE
/// ADD COLUMN` (`Cannot add a REFERENCES column with non-NULL default
/// value`), regardless of whether `NOT NULL` is also declared. Since
/// `with_triglavian_status` starts out `false` in Python itself, this
/// branch was almost certainly never actually run against a real
/// database -- the bug never surfaced.
///
/// The column here is left without an explicit `DEFAULT` (nullable,
/// implicit `NULL`) -- SQLite does allow that combination with
/// `REFERENCES`. `NULL` is the semantic equivalent of
/// `trigStatusID=0` ('None'): an unmarked system has no special status
/// either way. The FK stays active and validates normally (verified: a
/// value outside `mapTriglavianStatus`'s 5 rows is still rejected).
pub fn setup_triglavian_status(connection: &Connection) -> Result<(), BuilderError> {
    connection.execute_batch(
        "CREATE TABLE mapTriglavianStatus ( \
            trigStatusId INTEGER NOT NULL PRIMARY KEY, \
            trigStatusName TEXT NOT NULL \
         ) STRICT; \
         INSERT INTO mapTriglavianStatus (trigStatusId, trigStatusName) VALUES \
            (0, 'None'), \
            (1, 'Edencom Minor Victory'), \
            (2, 'Final Liminality'), \
            (3, 'Fortress'), \
            (4, 'Triglavian Minor Victory'); \
         ALTER TABLE mapSolarSystems ADD COLUMN trigStatusID \
            INTEGER REFERENCES mapTriglavianStatus(trigStatusID) \
            ON UPDATE CASCADE ON DELETE SET NULL; \
         CREATE INDEX trigStatus ON mapSolarSystems (solarSystemId, trigStatusID);",
    )?;

    for (status_id, ids) in [
        (1, EDENCOM_MINOR_VICTORY),
        (2, FINAL_LIMINALITY),
        (3, FORTRESS),
        (4, TRIGLAVIAN_MINOR_VICTORY),
    ] {
        let mut statement = connection
            .prepare("UPDATE mapSolarSystems SET trigStatusID = ?1 WHERE solarSystemId = ?2")?;
        for &solar_system_id in ids {
            statement.execute(rusqlite::params![status_id, solar_system_id])?;
        }
    }
    Ok(())
}

/// Adds `mapSolarSystems.joveObservatory` (with its index) and marks
/// the 1029 systems from [`JOVE_OBSERVATORY_SYSTEMS`]. Equivalent to
/// `create_jove_observatories()` in Python.
pub fn setup_jove_observatories(connection: &Connection) -> Result<(), BuilderError> {
    connection.execute_batch(
        "ALTER TABLE mapSolarSystems ADD COLUMN joveObservatory \
            INTEGER NOT NULL DEFAULT 0 CHECK (joveObservatory IN (0,1)); \
         CREATE INDEX joveSystems ON mapSolarSystems (solarSystemId, joveObservatory);",
    )?;

    let mut statement = connection
        .prepare("UPDATE mapSolarSystems SET joveObservatory = 1 WHERE solarSystemName = ?1")?;
    for name in JOVE_OBSERVATORY_SYSTEMS.lines() {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        statement.execute(rusqlite::params![name])?;
    }
    Ok(())
}

/// Adds `mapSolarSystems.specialOreAnom` and marks systems whose star
/// has spectral type "A0" (`typeStar.name`). Equivalent to
/// `create_special_anomalies()` in Python -- with an explicit `ts.name`
/// instead of the original's unqualified `name` (only apparently
/// ambiguous: `mapStars` has no `name` column, so it already resolved
/// to `typeStar.name` either way; it's made explicit here for clarity,
/// without changing the result).
pub fn setup_special_anomalies(connection: &Connection) -> Result<(), BuilderError> {
    connection.execute_batch(
        "ALTER TABLE mapSolarSystems ADD COLUMN specialOreAnom \
            INTEGER NOT NULL DEFAULT 0 CHECK (specialOreAnom IN (0,1));",
    )?;
    connection.execute(
        "UPDATE mapSolarSystems SET specialOreAnom = 1 WHERE solarSystemId IN ( \
            SELECT m.solarSystemId FROM typeStar AS ts \
            INNER JOIN mapStars AS m ON (ts.starTypeId = m.starTypeId) \
            WHERE ts.name = ?1 \
         )",
        rusqlite::params!["A0"],
    )?;
    Ok(())
}

/// Creates (and populates, where applicable) every community-data
/// table/column enabled by `config`. `mapAbstractSystems` always runs;
/// the rest respect each [`DotlanConfig`] flag. Equivalent to
/// `_update_tables()` in Python.
///
/// Note: unlike [`crate::builder::parser::parse_data`], this function
/// does NOT wrap the calls in an explicit transaction -- each
/// `CREATE TABLE`/`ALTER TABLE` is an independent DDL operation and
/// there's no circular FK between them that depends on seeing them all
/// together (unlike the `mapSystemGates.destinationGateId` case
/// documented in `parser::parse_stargates`). If the `mapAbstractSystems`
/// + SVG flow is added in a later phase, this is worth reconsidering.
pub fn update_tables(connection: &Connection, config: &DotlanConfig) -> Result<(), BuilderError> {
    create_abstract_map(connection)?;
    if config.with_icebelts {
        create_icebelts(connection)?;
    }
    if config.with_triglavian_status {
        setup_triglavian_status(connection)?;
    }
    if config.with_jove_observatories {
        setup_jove_observatories(connection)?;
    }
    if config.with_special_ore {
        setup_special_anomalies(connection)?;
    }
    Ok(())
}

/// Parses an already-downloaded dotlan SVG map (`map_path`), extracting
/// the ids of systems with an icebelt and the "abstract" coordinates of
/// each system for `mapAbstractSystems`. Equivalent to
/// `_extract_map_data()` in Python.
///
/// Returns `Ok(false)` -- not `Err` -- both if the file doesn't exist
/// and if the XML doesn't parse: same behavior as Python, where both
/// cases make the orchestrator (`process()`, still to be ported) retry
/// the download instead of aborting the whole build. A genuine `Err`
/// here can only come from a real SQLite error -- for example, if the
/// region name derived from the file doesn't match any row in
/// `mapRegions`, which violates `mapAbstractSystems.regionId`'s
/// `NOT NULL` (same behavior as Python's subquery `INSERT`: the
/// subquery `(SELECT regionId FROM mapRegions WHERE regionName=...)`
/// returns `NULL` if there's no match, and the `INSERT` fails).
///
/// The region name is derived from the file name, not received
/// separately: `The_Forge.svg` -> `"The Forge"` (underscores to
/// spaces) -- same as Python, except [`Path::file_stem`] is used
/// (strips everything after the LAST dot) instead of Python's
/// `split('.')[0]` (strips everything after the FIRST dot); for a
/// typical file name like this one, with no dots in the middle, both
/// give the same result.
///
/// # `id` format: confirmed against a real dotlan map
///
/// Verified against `Derelik.svg` (a real regional map, August 2026):
/// `<rect class="i">` elements use the `ice` prefix (11 out of 12 rects
/// with that class carried `id="iceNNNNNNNN"`; the remaining rect, with
/// no `id` at all, turned out to be a legend/visual-reference entry, not
/// a system -- confirming the `let Some(raw_id) = ... else { continue }`
/// guard is necessary, not over-defensive). `<use>` elements (abstract
/// systems) use the `sys` prefix -- all 125 `<use>` entries in the file
/// carried complete `id`/`x`/`y`, none missing. Both prefixes are 3
/// characters, matching Python's `tag_id[3::]` -- the code doesn't
/// depend on the prefix's TEXT (never compares it), it just discards 3
/// fixed bytes, so it works the same for `ice`/`sys` or any other
/// 3-character ASCII prefix that might show up in other regions.
///
/// If some other regional map carried a different format anyway, this
/// function doesn't break: any `id`/`x`/`y` that doesn't parse as a
/// number is simply skipped (with a warning on stderr), row by row,
/// without aborting the rest of the parsing.
///
/// Unlike Python (which passes the `id` as a plain string, relying on
/// SQLite's classic type coercion -- its `mapAbstractSystems` isn't
/// `STRICT`), here all three values (`solarSystemId`, `x`, `y`) are
/// explicitly parsed to `i64`/`f64` before binding them: this isn't a
/// style preference, it's a real requirement of this crate's `STRICT`
/// tables, which don't accept the implicit text-to-number coercion
/// classic SQLite does.
pub fn extract_map_data(
    connection: &Connection,
    map_path: &Path,
    config: &DotlanConfig,
) -> Result<bool, BuilderError> {
    if !map_path.exists() {
        eprintln!(
            "dotlan: {} doesn't exist, skipping parsing",
            map_path.display()
        );
        return Ok(false);
    }

    let content = std::fs::read_to_string(map_path)?;
    let doc = match roxmltree::Document::parse(&content) {
        Ok(doc) => doc,
        Err(err) => {
            eprintln!("dotlan: error parsing {} - {err}", map_path.display());
            return Ok(false);
        }
    };

    // --- icebelts: <rect class="i" id="..."> ---
    let mut icebelt_ids = Vec::new();
    for tag in doc
        .descendants()
        .filter(|n| n.has_tag_name((SVG_NS, "rect")) && n.attribute("class") == Some("i"))
    {
        let Some(raw_id) = tag.attribute("id") else {
            continue;
        };
        match raw_id.get(3..).and_then(|s| s.parse::<i64>().ok()) {
            Some(id) => icebelt_ids.push(id),
            None => eprintln!(
                "dotlan: unexpected icebelt id '{raw_id}' in {}, skipping",
                map_path.display()
            ),
        }
    }
    if !icebelt_ids.is_empty() && config.with_icebelts {
        let mut statement = connection
            .prepare("UPDATE mapSolarSystems SET iceBelt = 1 WHERE solarSystemId = ?1")?;
        for id in &icebelt_ids {
            statement.execute(rusqlite::params![id])?;
        }
    }

    // --- mapAbstractSystems: <use id="..." x="..." y="..."> ---
    let region_name = map_path
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.replace('_', " "))
        .ok_or_else(|| {
            BuilderError::Data(format!(
                "couldn't derive a region name from {}",
                map_path.display()
            ))
        })?;

    let mut insert_abstract = connection.prepare(
        "INSERT INTO mapAbstractSystems (solarSystemId, regionId, x, y) \
         VALUES (?1, (SELECT regionId FROM mapRegions WHERE regionName = ?2), ?3, ?4)",
    )?;
    for tag in doc
        .descendants()
        .filter(|n| n.has_tag_name((SVG_NS, "use")))
    {
        let (Some(raw_id), Some(raw_x), Some(raw_y)) =
            (tag.attribute("id"), tag.attribute("x"), tag.attribute("y"))
        else {
            continue;
        };
        let parsed = raw_id
            .get(3..)
            .and_then(|s| s.parse::<i64>().ok())
            .zip(raw_x.parse::<f64>().ok())
            .zip(raw_y.parse::<f64>().ok());
        let Some(((id, x), y)) = parsed else {
            eprintln!(
                "dotlan: unexpected <use id='{raw_id}' x='{raw_x}' y='{raw_y}'> in {}, skipping",
                map_path.display()
            );
            continue;
        };
        insert_abstract.execute(rusqlite::params![id, region_name, x, y])?;
    }

    Ok(true)
}

/// Fetches `(regionId, regionName)` for every "real" SDE region
/// (excludes w-space/abyssal, with `regionId >= 11000000`). Equivalent
/// to
/// `get_all_regions()` in Python -- except that here a SQL error
/// propagates as an `Err` instead of being printed and returning
/// `None`: Python catches `DatabaseError` right there and continues
/// with `rows = None` (which would then break `process()` some other
/// way when iterating over `None`), so there's no useful behavior to
/// replicate on that error path.
fn get_all_regions(connection: &Connection) -> Result<Vec<(i64, String)>, BuilderError> {
    let mut statement =
        connection.prepare("SELECT regionId, regionName FROM mapRegions WHERE regionId < ?1")?;
    let rows = statement
        .query_map(rusqlite::params![11_000_000_i64], |row| {
            Ok((row.get::<usize, i64>(0)?, row.get::<usize, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Downloads (with manifest-based caching) and parses the SVG map for
/// every "real" SDE region ([`get_all_regions`]), with up to 3 attempts
/// per region if parsing fails. Runs [`update_tables`] first -- same
/// order as Python, where `_update_tables()` is `process()`'s first
/// step, without exception. Equivalent to `process()` in Python.
///
/// `map_url_base` must end in `/` (e.g.
/// `"https://evemaps.dotlan.net/svg/"`) -- it's concatenated directly
/// with `<RegionName with underscores>.svg`, same as Python
/// (`self.map_url + region_name.replace(' ', '_') + ".svg"`, without
/// normalizing the trailing slash).
///
/// `sde_directory` is the builder's root working directory; the maps
/// and the manifest ([`manifest`]) live in `<sde_directory>/maps/`.
///
/// # Behavioral differences from Python
///
/// - If the download itself fails (network error, non-2xx HTTP
///   status) -- not just "the file came back corrupted" -- it's
///   treated the same as invalid data: logged and retried, same as
///   Python's `file_size is None` case (which its
///   `MiscUtils.download_file` essentially covers too).
/// - Deleting the file after a failed attempt uses
///   `let _ = std::fs::remove_file(...)`, ignoring the result --
///   Python first checks `.exists()` and then does `.unlink()` (which
///   could propagate a permission error uncaught); here it's
///   simplified to a silent attempt, acceptable given that, worst
///   case, an invalid file that couldn't be removed just ends up
///   getting overwritten on the next successful attempt anyway.
/// - `urlparse(map_url)` in Python does nothing observable (the result
///   is unused) -- not ported, it's dead code in the original.
pub async fn process(
    connection: &Connection,
    client: &Client,
    sde_directory: &Path,
    map_url_base: &str,
    config: &DotlanConfig,
) -> Result<(), BuilderError> {
    update_tables(connection, config)?;
    let regions = get_all_regions(connection)?;

    let maps_dir = sde_directory.join("maps");
    let mut manifest: Manifest = manifest::load(&maps_dir);
    let mut manifest_changed = false;

    for (_region_id, region_name) in regions {
        let file_name = format!("{}.svg", region_name.replace(' ', "_"));
        let map_path = maps_dir.join(&file_name);
        let map_url = format!("{map_url_base}{file_name}");

        let remote_fingerprint = http::fingerprint(client, &map_url).await;
        let mut needs_download = manifest::needs_download(
            map_path.exists(),
            manifest.get(&region_name),
            remote_fingerprint.as_ref(),
        );

        for attempt in 1..=3 {
            if needs_download {
                match http::download(client, &map_url, &map_path, |_| {}).await {
                    Ok(size) if size > 100 => {
                        println!("dotlan: map downloaded for {region_name}");
                        if let Some(fp) = &remote_fingerprint {
                            manifest.insert(region_name.clone(), fp.clone());
                            manifest_changed = true;
                        }
                    }
                    Ok(_) => {
                        let _ = std::fs::remove_file(&map_path);
                        eprintln!("dotlan: invalid data received for {region_name}");
                    }
                    Err(err) => {
                        let _ = std::fs::remove_file(&map_path);
                        eprintln!("dotlan: error downloading the map for {region_name}: {err}");
                    }
                }
            } else {
                println!("dotlan: {region_name} unchanged, skipping download.");
            }

            println!("dotlan: parsing data for {region_name}");
            if extract_map_data(connection, &map_path, config)? {
                break;
            }
            needs_download = true;
            let _ = std::fs::remove_file(&map_path);
            eprintln!("dotlan: invalid data for {region_name}, retrying download ({attempt}).");
        }
    }

    if manifest_changed {
        manifest::save(&maps_dir, &manifest)?;
    }

    Ok(())
}

const EDENCOM_MINOR_VICTORY: &[i64] = &[
    30003088, 30003894, 30004302, 30005074, 30003570, 30003463, 30003788, 30002724, 30002999,
    30000102, 30003919, 30004978, 30004287, 30002051, 30003823, 30005267, 30003587, 30003904,
    30005209, 30005219, 30002755, 30003824, 30002239, 30003794, 30003927, 30000109, 30000060,
    30000160, 30004999, 30004295, 30004231, 30004284, 30003932, 30004254, 30002513, 30002048,
    30003090, 30003478, 30004289, 30003061, 30003078, 30003900, 30002644, 30003480, 30001696,
    30002772, 30005284, 30005222, 30005086, 30003918, 30003908, 30000012, 30003481, 30003460,
    30005213, 30005308, 30003058, 30005334, 30002506, 30003931, 30005255, 30004263, 30000062,
    30002241, 30003558, 30001376, 30004257, 30004108, 30000048, 30003482, 30005263, 30005066,
    30004268, 30005236, 30003829, 30005034, 30003074, 30003809, 30001718, 30004256, 30004301,
    30002397, 30003854, 30001660,
];

const FINAL_LIMINALITY: &[i64] = &[
    30002079, 30002652, 30002411, 30005005, 30000021, 30002797, 30031392, 30001413, 30000206,
    30040141, 30045328, 30002770, 30003504, 30002737, 30000192, 30000157, 30001372, 30002702,
    30003046, 30020141, 30045329, 30002225, 30001381, 30001445, 30010141, 30005029, 30003495,
];

const FORTRESS: &[i64] = &[
    30003539, 30003573, 30005251, 30004103, 30000118, 30004090, 30003548, 30000113, 30002386,
    30004973, 30002266, 30002530, 30004141, 30002253, 30003398, 30003490, 30003556, 30002385,
    30002704, 30005058, 30004305, 30003883, 30003397, 30004084, 30003574, 30002665, 30000188,
    30003514, 30005052, 30000004, 30002242, 30005252, 30002986, 30002700, 30003050, 30000005,
    30004250, 30003392, 30003515, 30004100, 30002662, 30045322, 30003885, 30004248, 30003541,
    30002651, 30004150, 30002251, 30005260, 30000105, 30004992, 30002243, 30003553,
];

const TRIGLAVIAN_MINOR_VICTORY: &[i64] = &[
    30045331, 30001400, 30004244, 30045345, 30001358, 30001401, 30045354, 30002557, 30002760,
    30002795, 30004981, 30001447, 30001390, 30003076, 30000163, 30003073, 30001391, 30002771,
    30005330, 30000205, 30003856, 30002645, 30045338, 30002575, 30001383, 30003464, 30000182,
    30001685,
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Common prerequisites: full schema + one region/constellation and
    /// two solar systems (one with an id from the Edencom list, to test
    /// triglavian population; the other named like the first real
    /// system in `jove_observatories.txt`, to test exact matching by
    /// name).
    fn setup(connection: &Connection) {
        crate::builder::schema::create_schema(connection).unwrap();
        connection
            .execute(
                "INSERT INTO mapRegions (regionId, regionName, nebula, centerX, centerY, centerZ) \
                 VALUES (10000002, 'The Forge', 5, 0, 0, 0)",
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
        // 30003088 is the first id in EDENCOM_MINOR_VICTORY.
        connection
            .execute(
                "INSERT INTO mapSolarSystems \
                 (solarSystemId, solarSystemName, constellationId, radius, centerX, centerY, centerZ, security) \
                 VALUES (30003088, 'Sys Edencom', 20000020, 1.0, 0, 0, 0, 0.5)",
                [],
            )
            .unwrap();
        // "0-4VQL" is the first real line in jove_observatories.txt.
        connection
            .execute(
                "INSERT INTO mapSolarSystems \
                 (solarSystemId, solarSystemName, constellationId, radius, centerX, centerY, centerZ, security) \
                 VALUES (30000001, '0-4VQL', 20000020, 1.0, 0, 0, 0, 0.9)",
                [],
            )
            .unwrap();
    }

    /// Writes a temporary SVG with the exact name `file_name` (so
    /// `extract_map_data` derives the correct region name) inside a
    /// unique temp directory, and returns its path. The directory isn't
    /// removed automatically -- these are a few-byte files in
    /// `std::env::temp_dir()`, just as disposable as this crate's other
    /// fixtures.
    fn write_temp_svg(test_name: &str, file_name: &str, content: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "sde-dotlan-test-{test_name}-{}-{}",
            std::process::id(),
            file_name.len() // cheap disperser to avoid collisions between tests
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(file_name);
        std::fs::write(&path, content).unwrap();
        path
    }

    const SAMPLE_SVG: &str = concat!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\">",
        "<rect class=\"i\" id=\"sys30003088\"/>",
        "<rect class=\"j\" id=\"sys30000001\"/>",
        "<use id=\"sys30000001\" x=\"12.5\" y=\"-7.25\"/>",
        "</svg>",
    );

    #[test]
    fn extract_map_data_returns_false_when_file_missing() {
        let connection = Connection::open_in_memory().unwrap();
        setup(&connection);
        create_abstract_map(&connection).unwrap();

        let missing = std::env::temp_dir().join("sde-dotlan-test-no-existe.svg");
        let config = DotlanConfig::default();
        let result = extract_map_data(&connection, &missing, &config).unwrap();
        assert!(!result);
    }

    #[test]
    fn extract_map_data_returns_false_on_malformed_svg() {
        let connection = Connection::open_in_memory().unwrap();
        setup(&connection);
        create_abstract_map(&connection).unwrap();

        let path = write_temp_svg("malformed", "The_Forge.svg", "<svg><rect></svg>");
        let config = DotlanConfig::default();
        let result = extract_map_data(&connection, &path, &config).unwrap();
        assert!(!result);
    }

    #[test]
    fn extract_map_data_inserts_abstract_systems_with_derived_region_name() {
        let connection = Connection::open_in_memory().unwrap();
        setup(&connection);
        create_abstract_map(&connection).unwrap();

        let path = write_temp_svg("abstract_systems", "The_Forge.svg", SAMPLE_SVG);
        let config = DotlanConfig::default();
        let ok = extract_map_data(&connection, &path, &config).unwrap();
        assert!(ok);

        let (region_id, x, y): (i64, f64, f64) = connection
            .query_row(
                "SELECT regionId, x, y FROM mapAbstractSystems WHERE solarSystemId = 30000001",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        // "The_Forge.svg" -> region "The Forge" -> regionId 10000002 (from the fixture).
        assert_eq!(region_id, 10000002);
        assert_eq!((x, y), (12.5, -7.25));
    }

    #[test]
    fn extract_map_data_updates_icebelt_only_when_enabled() {
        let connection = Connection::open_in_memory().unwrap();
        setup(&connection);
        create_abstract_map(&connection).unwrap();
        create_icebelts(&connection).unwrap();

        let path = write_temp_svg("icebelt_disabled", "The_Forge.svg", SAMPLE_SVG);
        // with_icebelts=false (default): the rect gets parsed but NOT written.
        let config = DotlanConfig::default();
        extract_map_data(&connection, &path, &config).unwrap();
        let ice_belt: i64 = connection
            .query_row(
                "SELECT iceBelt FROM mapSolarSystems WHERE solarSystemId = 30003088",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(ice_belt, 0, "with_icebelts=false shouldn't write anything");

        let config_enabled = DotlanConfig {
            with_icebelts: true,
            ..config
        };
        extract_map_data(&connection, &path, &config_enabled).unwrap();
        let ice_belt: i64 = connection
            .query_row(
                "SELECT iceBelt FROM mapSolarSystems WHERE solarSystemId = 30003088",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(ice_belt, 1, "with_icebelts=true should mark it");
    }

    #[test]
    fn extract_map_data_skips_use_tags_missing_attributes() {
        let connection = Connection::open_in_memory().unwrap();
        setup(&connection);
        create_abstract_map(&connection).unwrap();

        let svg = concat!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\">",
            "<use id=\"sys30000001\" x=\"1.0\"/>", // missing y
            "<use x=\"1.0\" y=\"2.0\"/>",          // missing id
            "</svg>",
        );
        let path = write_temp_svg("incomplete_use", "The_Forge.svg", svg);
        let config = DotlanConfig::default();
        let ok = extract_map_data(&connection, &path, &config).unwrap();
        assert!(ok, "un <use> incompleto se omite, no aborta el parseo");

        let total: i64 = connection
            .query_row("SELECT COUNT(*) FROM mapAbstractSystems", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(total, 0);
    }

    #[test]
    fn extract_map_data_handles_real_dotlan_excerpt() {
        // EXACT textual excerpt from a real dotlan map (Derelik.svg,
        // August 2026) -- not hand-synthesized: the legend rect
        // without an `id`, two real icebelt rects (`ice` prefix), and
        // two real `<use>` elements (`sys` prefix), just as they
        // appear in the original file.
        let svg = concat!(
            "<svg version=\"1.1\" xmlns=\"http://www.w3.org/2000/svg\" ",
            "xmlns:xlink=\"http://www.w3.org/1999/xlink\">",
            "<rect x=\"872\" y=\"726\" rx=\"5.5\" ry=\"5.5\" width=\"15.4\" height=\"11\" class=\"i\" />",
            "<rect id=\"ice30000072\" x=\"1\" y=\"0.5\" rx=\"14\" ry=\"13\" width=\"56\" height=\"28\" class=\"i\" />",
            "<rect id=\"ice30000087\" x=\"1\" y=\"0.5\" rx=\"14\" ry=\"13\" width=\"56\" height=\"28\" class=\"i\" />",
            "<use id=\"sys30000071\" x=\"0\" y=\"500\" width=\"62.5\" height=\"30\" xlink:href=\"#def30000071\" />",
            "<use id=\"sys30000074\" x=\"0\" y=\"555\" width=\"62.5\" height=\"30\" xlink:href=\"#def30000074\" />",
            "</svg>",
        );

        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        connection
            .execute(
                "INSERT INTO mapRegions (regionId, regionName, nebula, centerX, centerY, centerZ) \
                 VALUES (10000005, 'Derelik', 5, 0, 0, 0)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO mapConstellations \
                 (constellationId, constellationName, regionId, centerX, centerY, centerZ) \
                 VALUES (20000005, 'Konora', 10000005, 0, 0, 0)",
                [],
            )
            .unwrap();
        for id in [30000071, 30000072, 30000074, 30000087] {
            connection
                .execute(
                    "INSERT INTO mapSolarSystems \
                     (solarSystemId, solarSystemName, constellationId, radius, centerX, centerY, centerZ, security) \
                     VALUES (?1, ?1, 20000005, 1.0, 0, 0, 0, 0.5)",
                    rusqlite::params![id],
                )
                .unwrap();
        }
        create_abstract_map(&connection).unwrap();
        create_icebelts(&connection).unwrap();

        let path = write_temp_svg("real_excerpt", "Derelik.svg", svg);
        let config = DotlanConfig {
            with_icebelts: true,
            ..DotlanConfig::default()
        };
        let ok = extract_map_data(&connection, &path, &config).unwrap();
        assert!(ok);

        // The legend rect (without an id) shouldn't generate any extra UPDATE
        // -- only the two with a real id end up marked.
        let ice_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM mapSolarSystems WHERE iceBelt = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(ice_count, 2);

        let (region_id, x, y): (i64, f64, f64) = connection
            .query_row(
                "SELECT regionId, x, y FROM mapAbstractSystems WHERE solarSystemId = 30000071",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(region_id, 10000005);
        assert_eq!((x, y), (0.0, 500.0));
    }

    /// Common prerequisites for `process()`'s tests: schema + "Test
    /// Region" (regionId 10000099, below the w-space/abyssal threshold)
    /// with two solar systems matching those in `SAMPLE_SVG`
    /// (30003088, 30000001), plus one abyssal region (regionId >=
    /// 11000000) to test `get_all_regions`'s filter. Returns a
    /// `Connection`.
    fn setup_for_process() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        connection
            .execute(
                "INSERT INTO mapRegions (regionId, regionName, nebula, centerX, centerY, centerZ) \
                 VALUES (10000099, 'Test Region', 5, 0, 0, 0)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO mapRegions (regionId, regionName, nebula, centerX, centerY, centerZ) \
                 VALUES (11000001, 'A-R00001', 5, 0, 0, 0)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO mapConstellations \
                 (constellationId, constellationName, regionId, centerX, centerY, centerZ) \
                 VALUES (20000099, 'Test Constellation', 10000099, 0, 0, 0)",
                [],
            )
            .unwrap();
        for id in [30003088, 30000001] {
            connection
                .execute(
                    "INSERT INTO mapSolarSystems \
                     (solarSystemId, solarSystemName, constellationId, radius, centerX, centerY, centerZ, security) \
                     VALUES (?1, ?1, 20000099, 1.0, 0, 0, 0, 0.5)",
                    rusqlite::params![id],
                )
                .unwrap();
        }
        connection
    }

    fn temp_sde_dir(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("sde-dotlan-process-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn get_all_regions_excludes_wormhole_and_abyssal() {
        let connection = setup_for_process();
        let regions = get_all_regions(&connection).unwrap();
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0], (10000099, "Test Region".to_string()));
    }

    #[tokio::test]
    async fn process_downloads_new_region_and_populates_database() {
        let connection = setup_for_process();
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("HEAD"))
            .and(wiremock::matchers::path("/Test_Region.svg"))
            .respond_with(wiremock::ResponseTemplate::new(200).insert_header("ETag", "\"v1\""))
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/Test_Region.svg"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(SAMPLE_SVG))
            .mount(&server)
            .await;

        let client = http::build_client().unwrap();
        let sde_dir = temp_sde_dir("new_region");
        let map_url_base = format!("{}/", server.uri());
        let config = DotlanConfig::default();

        process(&connection, &client, &sde_dir, &map_url_base, &config)
            .await
            .unwrap();

        let total: i64 = connection
            .query_row("SELECT COUNT(*) FROM mapAbstractSystems", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(total, 1);

        // The manifest should have been saved with the new fingerprint.
        let manifest = manifest::load(&sde_dir.join("maps"));
        assert!(manifest.contains_key("Test Region"));
        assert_eq!(manifest["Test Region"].etag.as_deref(), Some("\"v1\""));
    }

    #[tokio::test]
    async fn process_skips_download_when_manifest_matches() {
        let connection = setup_for_process();
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("HEAD"))
            .and(wiremock::matchers::path("/Test_Region.svg"))
            .respond_with(wiremock::ResponseTemplate::new(200).insert_header("ETag", "\"same\""))
            .mount(&server)
            .await;
        // GET should never be called -- explicitly expect 0.
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/Test_Region.svg"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(SAMPLE_SVG))
            .expect(0)
            .mount(&server)
            .await;

        let client = http::build_client().unwrap();
        let sde_dir = temp_sde_dir("unchanged");
        let maps_dir = sde_dir.join("maps");
        std::fs::create_dir_all(&maps_dir).unwrap();
        std::fs::write(maps_dir.join("Test_Region.svg"), SAMPLE_SVG).unwrap();

        // Manifest pre-populated with the SAME fingerprint the HEAD returns.
        let mut manifest: Manifest = Manifest::new();
        manifest.insert(
            "Test Region".to_string(),
            manifest::MapFingerprint {
                etag: Some("\"same\"".to_string()),
                last_modified: None,
                content_length: None,
            },
        );
        manifest::save(&maps_dir, &manifest).unwrap();

        let map_url_base = format!("{}/", server.uri());
        let config = DotlanConfig::default();
        process(&connection, &client, &sde_dir, &map_url_base, &config)
            .await
            .unwrap();

        // The local (already existing) file still gets parsed -- no download.
        let total: i64 = connection
            .query_row("SELECT COUNT(*) FROM mapAbstractSystems", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(total, 1);
        // wiremock verifies the GET's .expect(0) when the server drops.
    }

    #[tokio::test]
    async fn process_retries_after_invalid_download_then_succeeds() {
        let connection = setup_for_process();
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("HEAD"))
            .and(wiremock::matchers::path("/Test_Region.svg"))
            .respond_with(wiremock::ResponseTemplate::new(200).insert_header("ETag", "\"v1\""))
            .mount(&server)
            .await;
        // First GET: invalid body (<=100 bytes) -- forces a retry.
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/Test_Region.svg"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_string("x"))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        // From the second GET onward: valid content.
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/Test_Region.svg"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(SAMPLE_SVG))
            .mount(&server)
            .await;

        let client = http::build_client().unwrap();
        let sde_dir = temp_sde_dir("retry");
        let map_url_base = format!("{}/", server.uri());
        let config = DotlanConfig::default();

        process(&connection, &client, &sde_dir, &map_url_base, &config)
            .await
            .unwrap();

        let total: i64 = connection
            .query_row("SELECT COUNT(*) FROM mapAbstractSystems", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(total, 1, "should succeed after retrying the download");
    }

    #[test]
    fn jove_observatory_list_has_no_duplicates_and_no_blank_lines() {
        let names: Vec<&str> = JOVE_OBSERVATORY_SYSTEMS
            .lines()
            .filter(|l| !l.trim().is_empty())
            .collect();
        assert_eq!(names.len(), 1029);
        let unique: std::collections::HashSet<&str> = names.iter().copied().collect();
        assert_eq!(
            unique.len(),
            1029,
            "no duplicates should remain after deduplication"
        );
    }

    #[test]
    fn create_abstract_map_accepts_fractional_coords() {
        let connection = Connection::open_in_memory().unwrap();
        setup(&connection);

        create_abstract_map(&connection).unwrap();
        connection
            .execute(
                "INSERT INTO mapAbstractSystems (solarSystemId, regionId, x, y) \
                 VALUES (30000001, 10000002, 12.5, -7.25)",
                [],
            )
            .unwrap();

        let (x, y): (f64, f64) = connection
            .query_row(
                "SELECT x, y FROM mapAbstractSystems WHERE solarSystemId = 30000001",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!((x, y), (12.5, -7.25));
    }

    #[test]
    fn create_icebelts_adds_column_defaulting_to_zero() {
        let connection = Connection::open_in_memory().unwrap();
        setup(&connection);

        create_icebelts(&connection).unwrap();
        let ice_belt: i64 = connection
            .query_row(
                "SELECT iceBelt FROM mapSolarSystems WHERE solarSystemId = 30000001",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(ice_belt, 0);

        connection
            .execute(
                "UPDATE mapSolarSystems SET iceBelt = 1 WHERE solarSystemId = 30000001",
                [],
            )
            .unwrap();
        let ice_belt: i64 = connection
            .query_row(
                "SELECT iceBelt FROM mapSolarSystems WHERE solarSystemId = 30000001",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(ice_belt, 1);
    }

    #[test]
    fn setup_triglavian_status_marks_known_systems_and_defaults_to_null() {
        let connection = Connection::open_in_memory().unwrap();
        setup(&connection);

        setup_triglavian_status(&connection).unwrap();

        let status_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM mapTriglavianStatus", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(status_count, 5);

        // 30003088 is in EDENCOM_MINOR_VICTORY -> trigStatusID=1.
        let marked: Option<i64> = connection
            .query_row(
                "SELECT trigStatusID FROM mapSolarSystems WHERE solarSystemId = 30003088",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(marked, Some(1));

        // 30000001 isn't in any list -> NULL (not 0).
        let unmarked: Option<i64> = connection
            .query_row(
                "SELECT trigStatusID FROM mapSolarSystems WHERE solarSystemId = 30000001",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(unmarked, None);
    }

    #[test]
    fn setup_triglavian_status_foreign_key_still_enforced() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch("PRAGMA foreign_keys = ON;")
            .unwrap();
        setup(&connection);
        setup_triglavian_status(&connection).unwrap();

        let result = connection.execute(
            "UPDATE mapSolarSystems SET trigStatusID = 99 WHERE solarSystemId = 30000001",
            [],
        );
        assert!(result.is_err());
    }

    #[test]
    fn setup_jove_observatories_marks_by_exact_name() {
        let connection = Connection::open_in_memory().unwrap();
        setup(&connection);

        setup_jove_observatories(&connection).unwrap();

        // "0-4VQL" (30000001) is in the real list.
        let marked: i64 = connection
            .query_row(
                "SELECT joveObservatory FROM mapSolarSystems WHERE solarSystemId = 30000001",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(marked, 1);

        // "Sys Edencom" (30003088) is not in the list.
        let unmarked: i64 = connection
            .query_row(
                "SELECT joveObservatory FROM mapSolarSystems WHERE solarSystemId = 30003088",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(unmarked, 0);
    }

    #[test]
    fn setup_special_anomalies_marks_a0_spectral_class_systems() {
        let connection = Connection::open_in_memory().unwrap();
        setup(&connection);
        connection
            .execute(
                "INSERT INTO invCategories (categoryId, categoryName, published) VALUES (6,'Celestial',1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO invGroups (groupId, categoryId, groupName, anchorable) VALUES (6,6,'Sun',0)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO invTypes (typeId, groupId, typeName, published) VALUES (3000,6,'White A0 (ffffff)',1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO typeStar (typeId, name, color) VALUES (3000,'A0','ffffff')",
                [],
            )
            .unwrap();
        let star_type_id: i64 = connection
            .query_row(
                "SELECT starTypeId FROM typeStar WHERE typeId = 3000",
                [],
                |row| row.get(0),
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO mapStars (starId, solarSystemId, radius, starTypeId) \
                 VALUES (40000001, 30000001, 1000, ?1)",
                rusqlite::params![star_type_id],
            )
            .unwrap();

        setup_special_anomalies(&connection).unwrap();

        let marked: i64 = connection
            .query_row(
                "SELECT specialOreAnom FROM mapSolarSystems WHERE solarSystemId = 30000001",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(marked, 1);

        let unmarked: i64 = connection
            .query_row(
                "SELECT specialOreAnom FROM mapSolarSystems WHERE solarSystemId = 30003088",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(unmarked, 0);
    }

    #[test]
    fn update_tables_only_creates_what_config_enables() {
        let connection = Connection::open_in_memory().unwrap();
        setup(&connection);

        let config = DotlanConfig {
            with_icebelts: false,
            with_triglavian_status: false,
            with_jove_observatories: false,
            with_special_ore: false,
        };
        update_tables(&connection, &config).unwrap();

        // mapAbstractSystems corre siempre.
        let abstract_exists: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='mapAbstractSystems'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(abstract_exists, 1);

        // None of the 4 optional columns should exist.
        let columns: Vec<String> = connection
            .prepare("SELECT name FROM pragma_table_info('mapSolarSystems')")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        for optional in [
            "iceBelt",
            "trigStatusID",
            "joveObservatory",
            "specialOreAnom",
        ] {
            assert!(
                !columns.iter().any(|c| c == optional),
                "{optional} shouldn't exist with every flag set to false"
            );
        }
    }

    #[test]
    fn update_tables_creates_everything_when_all_enabled() {
        let connection = Connection::open_in_memory().unwrap();
        setup(&connection);

        let config = DotlanConfig {
            with_icebelts: true,
            with_triglavian_status: true,
            with_jove_observatories: true,
            with_special_ore: true,
        };
        update_tables(&connection, &config).unwrap();

        let columns: Vec<String> = connection
            .prepare("SELECT name FROM pragma_table_info('mapSolarSystems')")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        for expected in [
            "iceBelt",
            "trigStatusID",
            "joveObservatory",
            "specialOreAnom",
        ] {
            assert!(
                columns.iter().any(|c| c == expected),
                "missing column {expected}"
            );
        }
    }
}
