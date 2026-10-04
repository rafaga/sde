//! Keeping `sde.db` current with sde-deltas instead of CCP's full export.
//!
//! [`prepare`] brings the [`Mirror`] kept as a single zip up to the latest
//! build sde-deltas publishes, and says what `sde.db` needs as an
//! [`UpdatePlan`]:
//!
//! - [`UpdatePlan::UpToDate`] -- nothing.
//! - [`UpdatePlan::Bump`] -- none of the changes touch anything the parser
//!   reads, so only the build recorded in `sdeFingerprint` moves
//!   ([`set_sde_build`]). No download beyond the deltas' manifests, no
//!   rebuild.
//! - [`UpdatePlan::Rebuild`] -- rebuild `sde.db` with
//!   [`super::parser::Parser`] from the mirror, unpacked in the SDE
//!   directory (removed afterwards with [`release_working_copy`]): local, no
//!   download of CCP's export.
//! - [`UpdatePlan::Full`] -- deltas can't be used (no mirror yet, the chain
//!   doesn't reach the mirror's build, a schema change, drift, network...);
//!   build from CCP's full export as before, then [`create_mirror`] so the
//!   next update can use deltas.
//!
//! sde-deltas knows nothing about `sde.db`: all the decisions are made here,
//! from what [`super::usage`] recorded the parser reading.

use crate::Error;
use crate::SdeManager;
use crate::builder::BuildUrls;
use crate::builder::deltas;
use crate::builder::mirror::{Mirror, MirrorMeta, SchemaAlarm};
use crate::builder::parser::{PARSER_OUTPUT_VERSION, Parser, ParserConfig};
use crate::builder::{extract, sde_index};
use crate::objects::SdeFingerprint;
use reqwest::Client;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// After this many builds applied from deltas, the next update is a full
/// build: an inexpensive safety net against drift the deltas can't reveal
/// (e.g. a field the parser only reads for data that didn't exist when the
/// mirror was created).
pub const REFRESH_AFTER_DELTA_BUILDS: u32 = 100;

/// How long sde-deltas may lag behind CCP (since the release of the first
/// build it lacks) before a full build is preferred over waiting for its
/// delta.
pub const MAX_DELTA_LAG_SECONDS: i64 = 2 * 24 * 60 * 60;

/// How many of CCP's changelogs [`prepare`] reads back from CCP's latest
/// build looking for the first build sde-deltas lacks. Further behind than
/// this counts as lagging.
const LAG_WALK_LIMIT: usize = 20;

/// Options for [`prepare`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpdateOptions {
    /// See [`MAX_DELTA_LAG_SECONDS`]. `None` never gives up on sde-deltas
    /// for lagging: the database stays at sde-deltas' latest build until
    /// it catches up.
    pub max_delta_lag_seconds: Option<i64>,
}

impl Default for UpdateOptions {
    fn default() -> Self {
        Self {
            max_delta_lag_seconds: Some(MAX_DELTA_LAG_SECONDS),
        }
    }
}

/// What `sde.db` needs. See the module docs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdatePlan {
    UpToDate {
        build: String,
    },
    /// `from` is `sde.db`'s current build.
    Bump {
        from: String,
        to: String,
    },
    /// `from` is `sde.db`'s current build, `None` if it has no usable one
    /// (missing, invalid, or built with another config).
    Rebuild {
        from: Option<String>,
        to: String,
        mirror_dir: PathBuf,
    },
    Full {
        reason: FullReason,
    },
}

/// Why deltas couldn't be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FullReason {
    /// The SDE directory has no usable mirror (first run, or a crash while
    /// one was being written).
    NoMirror,
    /// The mirror was projected by another version of this crate, or for a
    /// config that reads other fields.
    MirrorMismatch,
    /// [`REFRESH_AFTER_DELTA_BUILDS`] reached.
    Refresh,
    /// sde-deltas has no chain from the mirror's build.
    OutOfCoverage { build: String },
    /// sde-deltas is still at `deltas` while CCP, now at `ccp`, released
    /// the next build longer ago than [`UpdateOptions::max_delta_lag_seconds`].
    Lagging { ccp: String, deltas: u64 },
    /// A delta renames or drops something the parser reads: its mapping may
    /// need attention, and only a full build can tell.
    SchemaChanged(Vec<SchemaAlarm>),
    /// The deltas don't fit the mirror.
    Drift(Vec<String>),
    /// sde-deltas couldn't be read or applied (network, format...).
    Unavailable(String),
}

impl std::fmt::Display for FullReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FullReason::NoMirror => write!(f, "no local mirror of the SDE yet"),
            FullReason::MirrorMismatch => {
                write!(f, "the local mirror was made by another version or config")
            }
            FullReason::Refresh => write!(
                f,
                "{REFRESH_AFTER_DELTA_BUILDS} builds applied from deltas, refreshing"
            ),
            FullReason::OutOfCoverage { build } => {
                write!(f, "no chain of deltas from build {build}")
            }
            FullReason::Lagging { ccp, deltas } => {
                write!(f, "deltas stop at build {deltas}, CCP is at {ccp}")
            }
            FullReason::SchemaChanged(alarms) => {
                write!(f, "schema change in fields the parser reads: ")?;
                let alarms: Vec<String> = alarms.iter().map(ToString::to_string).collect();
                write!(f, "{}", alarms.join("; "))
            }
            FullReason::Drift(drift) => write!(
                f,
                "the deltas don't fit the local mirror ({} problems, first: {})",
                drift.len(),
                drift.first().map_or("", String::as_str)
            ),
            FullReason::Unavailable(error) => write!(f, "deltas unavailable: {error}"),
        }
    }
}

/// Progress of [`prepare`]: `done` of `total` builds applied, the next one
/// being `build`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeltaProgress {
    pub done: usize,
    pub total: usize,
    pub build: u64,
}

/// Brings the mirror stored in `archive` up to sde-deltas' latest build and
/// says what `db_path` needs, for a database built with `config`. See the
/// module docs. Never fails: anything that keeps deltas from being used is an
/// [`UpdatePlan::Full`].
///
/// The mirror is only unpacked, into `sde_dir`, when deltas have to be
/// applied or the database rebuilt; nothing is unpacked to find out that
/// the database is up to date. Whatever was unpacked is removed again
/// (`sde_dir` is emptied except for `maps/`) before returning, except for a
/// [`UpdatePlan::Rebuild`], which builds from it: the caller removes it with
/// [`release_working_copy`] afterwards, whether or not the build worked.
///
/// The archive is rewritten before returning when deltas were applied, so a
/// rebuild that then fails is retried by the next call
/// ([`UpdatePlan::Rebuild`] again, since `sde.db` is still behind the
/// mirror) without downloading anything.
///
/// A mirror an earlier version left unpacked in `sde_dir` (0.6.x) is packed
/// into `archive` the first time.
#[tracing::instrument(skip(client, on_progress))]
#[allow(clippy::too_many_arguments)]
pub async fn prepare(
    client: &Client,
    urls: &BuildUrls,
    db_path: &Path,
    sde_dir: &Path,
    archive: &Path,
    config: &ParserConfig,
    options: UpdateOptions,
    on_progress: impl FnMut(DeltaProgress),
) -> UpdatePlan {
    let mut unpacked = false;
    let plan = plan_update(
        client,
        urls,
        db_path,
        sde_dir,
        archive,
        config,
        options,
        on_progress,
        &mut unpacked,
    )
    .await;
    if unpacked && !matches!(plan, UpdatePlan::Rebuild { .. }) {
        let _ = release_working_copy(sde_dir);
    }
    plan
}

/// Removes the working copy of the mirror [`prepare`] unpacked into
/// `sde_dir` for an [`UpdatePlan::Rebuild`]: everything in it except
/// `maps/`.
pub fn release_working_copy(sde_dir: &Path) -> Result<(), Error> {
    extract::clean_except_maps(sde_dir)
}

/// [`prepare`] without the clean-up; `unpacked` tells whether `sde_dir`
/// got a working copy.
#[allow(clippy::too_many_arguments)]
async fn plan_update(
    client: &Client,
    urls: &BuildUrls,
    db_path: &Path,
    sde_dir: &Path,
    archive: &Path,
    config: &ParserConfig,
    options: UpdateOptions,
    mut on_progress: impl FnMut(DeltaProgress),
    unpacked: &mut bool,
) -> UpdatePlan {
    let full = |reason| UpdatePlan::Full { reason };
    if !archive.exists()
        && let Some(legacy) = Mirror::open(sde_dir)
    {
        // 0.6.x left the mirror unpacked in the SDE directory.
        if legacy.pack(archive).is_err() {
            return full(FullReason::NoMirror);
        }
        *unpacked = true;
    }
    let Some(meta) = Mirror::archive_meta(archive) else {
        return full(FullReason::NoMirror);
    };
    if !meta.matches(config) {
        return full(FullReason::MirrorMismatch);
    }
    if meta.delta_builds >= REFRESH_AFTER_DELTA_BUILDS {
        return full(FullReason::Refresh);
    }
    let Ok(from) = meta.build.parse::<u64>() else {
        return full(FullReason::OutOfCoverage {
            build: meta.build.clone(),
        });
    };

    let index = match deltas::fetch_index(client, &urls.deltas_url).await {
        Ok(index) => index,
        Err(error) => return full(FullReason::Unavailable(error.to_string())),
    };
    // A full build right after CCP releases a build leaves the mirror ahead
    // of sde-deltas until it publishes that build's delta: nothing to apply
    // then, and nothing to download either -- the mirror already has it.
    let deltas_latest = index.latest_build.unwrap_or(0);
    let reached = deltas_latest.max(from);
    if let Some(max_lag) = options.max_delta_lag_seconds
        && let Some(ccp) = lagging_behind(client, &urls.sde_url, reached, max_lag).await
    {
        return full(FullReason::Lagging {
            ccp,
            deltas: deltas_latest,
        });
    }
    let chain = if from >= deltas_latest {
        Vec::new()
    } else {
        match index.chain(from) {
            Some(chain) => chain,
            None => {
                return full(FullReason::OutOfCoverage {
                    build: meta.build.clone(),
                });
            }
        }
    };

    // Nothing to apply and the database is already there: nothing to unpack.
    if chain.is_empty()
        && let Some(build) = installed_build(db_path, config)
        && build == meta.build
    {
        return UpdatePlan::UpToDate { build };
    }

    *unpacked = true;
    let mirror = match Mirror::unpack(archive, sde_dir) {
        Ok(mirror) => mirror,
        Err(error) => return full(FullReason::Unavailable(error.to_string())),
    };
    let total = chain.len();
    let mut update = mirror.begin();
    // Record counts of the last build applied, when sde-deltas published them.
    let mut counts = None;
    for (done, entry) in chain.iter().enumerate() {
        on_progress(DeltaProgress {
            done,
            total,
            build: entry.build,
        });
        let (lines, build_counts) =
            match fetch_lines(client, &urls.deltas_url, entry, &mirror).await {
                Ok(fetched) => fetched,
                Err(error) => return full(FullReason::Unavailable(error.to_string())),
            };
        if let Err(error) = update.apply(&entry.build.to_string(), lines) {
            return full(FullReason::Unavailable(error.to_string()));
        }
        if !update.report().is_clean() {
            break;
        }
        counts = build_counts;
    }
    if update.report().is_clean()
        && let Some(counts) = &counts
        && let Err(error) = update.check_counts(counts)
    {
        return full(FullReason::Unavailable(error.to_string()));
    }
    let report = update.report().clone();
    if !report.alarms.is_empty() {
        return full(FullReason::SchemaChanged(report.alarms));
    }
    if !report.drift.is_empty() {
        return full(FullReason::Drift(report.drift));
    }
    let committed = if total > 0 {
        match update.commit() {
            Ok(committed) => {
                // The archive is the mirror that counts: the unpacked copy
                // goes away. A failure here keeps the previous archive.
                if let Err(error) = committed.pack(archive) {
                    return full(FullReason::Unavailable(error.to_string()));
                }
                Some(committed)
            }
            Err(error) => return full(FullReason::Unavailable(error.to_string())),
        }
    } else {
        drop(update);
        None
    };
    let mirror = committed.unwrap_or(mirror);
    if total > 0 {
        on_progress(DeltaProgress {
            done: total,
            total,
            build: index.latest_build.unwrap_or(from),
        });
    }

    let to = mirror.build().to_string();
    match installed_build(db_path, config) {
        Some(build) if build == to => UpdatePlan::UpToDate { build },
        Some(build) if !report.relevant && build == from.to_string() => {
            UpdatePlan::Bump { from: build, to }
        }
        build => UpdatePlan::Rebuild {
            from: build,
            to,
            mirror_dir: mirror.dir().to_path_buf(),
        },
    }
}

/// The lines of `entry`'s delta (none when its manifest shows it only
/// touches tables the mirror doesn't hold: the delta isn't downloaded), and
/// the record counts its manifest publishes, if any.
async fn fetch_lines(
    client: &Client,
    base_url: &str,
    entry: &deltas::IndexEntry,
    mirror: &Mirror,
) -> Result<(Vec<serde_json::Value>, Option<BTreeMap<String, u64>>), Error> {
    let manifest = deltas::fetch_manifest(client, base_url, entry.build).await?;
    if manifest.last_build != entry.last_build {
        return Err(Error::data(format!(
            "build {}: the manifest starts from {}, the index from {}",
            entry.build, manifest.last_build, entry.last_build
        )));
    }
    let lines = if manifest
        .touched_tables()
        .any(|table| mirror.usage().uses_table(table))
    {
        deltas::fetch_delta(client, base_url, &manifest).await?
    } else {
        Vec::new()
    };
    Ok((lines, manifest.counts))
}

/// `sde.db`'s build, if its fingerprint is intact and was written for the
/// same config by a parser with the same [`PARSER_OUTPUT_VERSION`] --
/// otherwise it has to be rebuilt (from the mirror) anyway.
pub fn installed_build(db_path: &Path, config: &ParserConfig) -> Option<String> {
    let manager = SdeManager::new(db_path, 1.0).ok()?;
    let (fingerprint, intact) = manager.get_fingerprint().ok()??;
    let current = intact
        && fingerprint_matches(&fingerprint, config)
        && output_version(db_path) == Some(PARSER_OUTPUT_VERSION);
    current.then_some(fingerprint.sde_build)?
}

/// `sde.db`'s `PRAGMA user_version`: the [`PARSER_OUTPUT_VERSION`] of the
/// parser that built it.
fn output_version(db_path: &Path) -> Option<u32> {
    let connection =
        rusqlite::Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .ok()?;
    connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .ok()
}

fn fingerprint_matches(fingerprint: &SdeFingerprint, config: &ParserConfig) -> bool {
    fingerprint.language == config.language
        && fingerprint.position_2d == config.position_2d
        && fingerprint.map_kspace == config.map_kspace
        && fingerprint.map_wspace == config.map_wspace
        && fingerprint.map_abyssal == config.map_abyssal
        && fingerprint.map_void == config.map_void
        && fingerprint.with_gates == config.with_gates
        && fingerprint.with_moons == config.with_moons
        && fingerprint.with_third_party == config.with_third_party
}

/// Records `build` in `sde.db`'s fingerprint and recomputes its hash, for
/// [`UpdatePlan::Bump`]. Refuses a database whose fingerprint is missing or
/// doesn't verify: that one needs a rebuild instead.
#[tracing::instrument]
pub fn set_sde_build(db_path: &Path, build: &str) -> Result<(), Error> {
    let manager = SdeManager::new(db_path, 1.0)?;
    let Some((mut fingerprint, true)) = manager.get_fingerprint()? else {
        return Err(Error::data(format!(
            "{db_path:?} has no intact fingerprint to move to build {build}"
        )));
    };
    fingerprint.sde_build = Some(build.to_string());
    let connection = rusqlite::Connection::open(db_path)?;
    connection.execute(
        "UPDATE sdeFingerprint SET sdeBuild = ?1, hash = ?2 WHERE id = 1",
        rusqlite::params![build, fingerprint.hash()],
    )?;
    Ok(())
}

/// Turns the full export `parser` just read in `sde_dir` into the mirror
/// for the next update: reduces it ([`Mirror::create`]), stores it as the
/// single zip `archive` ([`Mirror::pack`]) and empties `sde_dir` except for
/// `maps/`. Call it after a successful full build. If it fails, `sde_dir`
/// may be left half reduced: empty it ([`release_working_copy`]).
pub fn create_mirror(
    sde_dir: &Path,
    archive: &Path,
    parser: &Parser,
    build: &str,
) -> Result<MirrorMeta, Error> {
    let usage = parser
        .field_usage()
        .ok_or_else(|| Error::data("the parser hasn't read the SDE yet"))?;
    let mirror = Mirror::create(sde_dir, &usage, build, parser.config())?;
    mirror.pack(archive)?;
    release_working_copy(sde_dir)?;
    Ok(mirror.meta().clone())
}

/// CCP's latest build, if sde-deltas (at `deltas_latest`) lacks a build CCP
/// released more than `max_lag` seconds ago. Walks CCP's changelogs back
/// from its latest build to find the first build sde-deltas lacks, up to
/// [`LAG_WALK_LIMIT`] of them (further behind counts as lagging). Best
/// effort: `None` when CCP's side can't be read.
async fn lagging_behind(
    client: &Client,
    sde_url: &str,
    deltas_latest: u64,
    max_lag: i64,
) -> Option<String> {
    let latest = sde_index::fetch_latest(client, sde_url).await.ok()??;
    let mut current = latest.build.parse::<u64>().ok()?;
    if current <= deltas_latest {
        return None;
    }
    for _ in 0..LAG_WALK_LIMIT {
        let meta = sde_index::fetch_changes_meta(client, sde_url, current)
            .await
            .ok()?;
        if meta.last_build <= deltas_latest {
            let released = unix_seconds(meta.release_date.as_deref()?)?;
            return (now() - released > max_lag).then_some(latest.build);
        }
        current = meta.last_build;
    }
    Some(latest.build)
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}

/// Seconds since the Unix epoch of an RFC 3339 UTC timestamp
/// (`2026-10-02T11:08:57Z`; fractions and offsets are ignored).
fn unix_seconds(timestamp: &str) -> Option<i64> {
    let number = |range: std::ops::Range<usize>| timestamp.get(range)?.parse::<i64>().ok();
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    // Days from the civil date (Howard Hinnant's `days_from_civil`).
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    Some(days * 86_400 + hour * 3_600 + minute * 60 + second)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_seconds_matches_known_timestamps() {
        assert_eq!(unix_seconds("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(unix_seconds("2000-03-01T00:00:00Z"), Some(951_868_800));
        assert_eq!(unix_seconds("2026-10-02T11:08:57Z"), Some(1_790_939_337));
        assert_eq!(unix_seconds("garbage"), None);
    }

    /// A CCP server whose latest build is 30, with changelogs 30 -> 25 -> 20.
    async fn ccp(release_of_25: &str) -> wiremock::MockServer {
        use wiremock::matchers::path;
        use wiremock::{Mock, ResponseTemplate};
        let server = wiremock::MockServer::start().await;
        let body = |text: String| ResponseTemplate::new(200).set_body_string(text);
        Mock::given(path("/latest.jsonl"))
            .respond_with(body(
                "{\"_key\":\"sde\",\"buildNumber\":30,\"releaseDate\":\"2999-01-01T00:00:00Z\"}"
                    .to_string(),
            ))
            .mount(&server)
            .await;
        for (build, last, date) in [(30, 25, "2999-01-01T00:00:00Z"), (25, 20, release_of_25)] {
            Mock::given(path(format!("/changes/{build}.jsonl")))
                .respond_with(body(format!(
                    "{{\"_key\":\"_meta\",\"buildNumber\":{build},\"lastBuildNumber\":{last},\
                     \"releaseDate\":\"{date}\"}}"
                )))
                .mount(&server)
                .await;
        }
        server
    }

    #[tokio::test]
    async fn lagging_is_measured_from_the_first_missing_build() {
        let client = Client::new();
        let old = ccp("2020-01-01T00:00:00Z").await;
        let url = format!("{}/", old.uri());
        // 25 is the first build sde-deltas (at 20) lacks, released long ago.
        assert_eq!(
            lagging_behind(&client, &url, 20, MAX_DELTA_LAG_SECONDS).await,
            Some("30".to_string())
        );
        // Up to date with CCP.
        assert_eq!(
            lagging_behind(&client, &url, 30, MAX_DELTA_LAG_SECONDS).await,
            None
        );
        // CCP's changelog chain can't be followed (no changes/20.jsonl):
        // best effort, not lagging.
        assert_eq!(
            lagging_behind(&client, &url, 5, MAX_DELTA_LAG_SECONDS).await,
            None
        );

        let recent = ccp("2999-01-01T00:00:00Z").await;
        let url = format!("{}/", recent.uri());
        assert_eq!(
            lagging_behind(&client, &url, 20, MAX_DELTA_LAG_SECONDS).await,
            None
        );
    }

    // -----------------------------------------------------------------
    // prepare(), against a mirror of a single `types` table reading
    // `name`, a database fingerprinted at build 1, and fake sde-deltas
    // and CCP servers.
    // -----------------------------------------------------------------

    use crate::builder::usage::FieldUsage;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use sha2::{Digest, Sha256};
    use std::io::Write;
    use wiremock::matchers::path;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    struct Setup {
        dir: PathBuf,
        server: MockServer,
    }

    impl Drop for Setup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// Reduces the export in `sde` to a mirror of build `build`, stores it
    /// in `archive` and empties `sde`.
    fn create_mirror_archive(sde: &Path, archive: &Path, usage: &FieldUsage, build: &str) {
        let mirror = Mirror::create(sde, usage, build, &ParserConfig::default()).unwrap();
        mirror.pack(archive).unwrap();
        release_working_copy(sde).unwrap();
    }

    impl Setup {
        fn db(&self) -> PathBuf {
            self.dir.join("sde.db")
        }

        fn sde(&self) -> PathBuf {
            self.dir.join("sde")
        }

        fn archive(&self) -> PathBuf {
            self.dir.join("data").join("sde-mirror.zip")
        }

        /// The build of the mirror in the archive.
        fn mirror_build(&self) -> String {
            Mirror::archive_meta(&self.archive()).unwrap().build
        }

        /// Changes the metadata of the mirror in the archive.
        fn edit_mirror(&self, edit: impl FnOnce(&mut MirrorMeta)) {
            let unpacked = Mirror::unpack(&self.archive(), &self.sde()).unwrap();
            let mut meta = unpacked.meta().clone();
            edit(&mut meta);
            std::fs::write(
                self.sde().join(crate::builder::mirror::META_FILE),
                serde_json::to_vec(&meta).unwrap(),
            )
            .unwrap();
            Mirror::open(&self.sde())
                .unwrap()
                .pack(&self.archive())
                .unwrap();
            release_working_copy(&self.sde()).unwrap();
        }

        /// Whether the SDE directory has nothing but `maps/` in it.
        fn sde_is_empty(&self) -> bool {
            std::fs::read_dir(self.sde())
                .unwrap()
                .all(|entry| entry.unwrap().file_name() == "maps")
        }

        fn urls(&self) -> BuildUrls {
            BuildUrls {
                sde_url: format!("{}/ccp/", self.server.uri()),
                deltas_url: format!("{}/deltas/", self.server.uri()),
                ..BuildUrls::default()
            }
        }

        async fn prepare(&self) -> UpdatePlan {
            prepare(
                &Client::new(),
                &self.urls(),
                &self.db(),
                &self.sde(),
                &self.archive(),
                &ParserConfig::default(),
                UpdateOptions::default(),
                |_| {},
            )
            .await
        }
    }

    fn fingerprint(build: &str) -> SdeFingerprint {
        let config = ParserConfig::default();
        SdeFingerprint {
            sde_build: Some(build.to_string()),
            language: config.language.clone(),
            position_2d: config.position_2d,
            map_kspace: config.map_kspace,
            map_wspace: config.map_wspace,
            map_abyssal: config.map_abyssal,
            map_void: config.map_void,
            with_gates: config.with_gates,
            with_moons: config.with_moons,
            with_third_party: config.with_third_party,
            with_icebelts: None,
            with_triglavian_status: None,
            with_jove_observatories: None,
            with_special_ore: None,
        }
    }

    fn gzip(text: &str) -> Vec<u8> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(text.as_bytes()).unwrap();
        encoder.finish().unwrap()
    }

    /// Mirror and database at build 1, and a server publishing build 2 as
    /// the latest (for both sde-deltas and CCP), whose manifest lists
    /// `tables` and whose delta is `delta` (with `sha256` as its published
    /// hash, or the right one).
    async fn setup(name: &str, tables: &str, delta: &str, sha256: Option<&str>) -> Setup {
        static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "sde_update_test_{name}_{}_{}",
            std::process::id(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        let sde = dir.join("sde");
        std::fs::create_dir_all(&sde).unwrap();
        std::fs::write(
            sde.join("types.jsonl"),
            "{\"_key\":1,\"name\":{\"en\":\"A\"},\"extra\":1}\n",
        )
        .unwrap();
        let mut usage = FieldUsage::default();
        usage.insert("types", "name");
        // Kept the way a build leaves it: one zip, and the SDE directory
        // emptied (see `legacy_mirrors_are_packed...` for the unpacked one).
        create_mirror_archive(&sde, &dir.join("data").join("sde-mirror.zip"), &usage, "1");

        let connection = rusqlite::Connection::open(dir.join("sde.db")).unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        let fingerprint = fingerprint("1");
        let (force, axis) = fingerprint.position_2d.fingerprint_columns();
        connection
            .execute(
                "INSERT INTO sdeFingerprint (id, sdeBuild, language, forceIsometricPosition2d, \
                 isometricProjectedAxis, mapKspace, mapWspace, mapAbyssal, mapVoid, withGates, \
                 withMoons, withThirdParty, hash) \
                 VALUES (1, '1', ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                rusqlite::params![
                    fingerprint.language,
                    force,
                    axis,
                    fingerprint.map_kspace,
                    fingerprint.map_wspace,
                    fingerprint.map_abyssal,
                    fingerprint.map_void,
                    fingerprint.with_gates,
                    fingerprint.with_moons,
                    fingerprint.with_third_party,
                    fingerprint.hash(),
                ],
            )
            .unwrap();

        let server = MockServer::start().await;
        let compressed = gzip(delta);
        let digest: String = Sha256::digest(&compressed)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let routes = [
            (
                "/deltas/index.json",
                r#"{"formatVersion":1,"firstBuild":2,"latestBuild":2,"builds":[
                    {"build":2,"lastBuild":1,"releaseDate":"2999-01-01T00:00:00Z","verification":"ok"}]}"#
                    .as_bytes()
                    .to_vec(),
            ),
            (
                "/deltas/2/manifest.json",
                format!(
                    r#"{{"formatVersion":1,"build":2,"lastBuild":1,"tables":{tables},
                        "files":{{"delta.jsonl.gz":{{"bytes":1,"sha256":"{}"}}}}}}"#,
                    sha256.unwrap_or(&digest)
                )
                .into_bytes(),
            ),
            ("/deltas/2/delta.jsonl.gz", compressed),
            (
                "/ccp/latest.jsonl",
                b"{\"_key\":\"sde\",\"buildNumber\":2,\"releaseDate\":\"2999-01-01T00:00:00Z\"}"
                    .to_vec(),
            ),
        ];
        for (route, body) in routes {
            Mock::given(path(route))
                .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
                .mount(&server)
                .await;
        }
        Setup { dir, server }
    }

    #[tokio::test]
    async fn a_build_touching_no_read_table_only_bumps() {
        let setup = setup("bump_table", r#"{"skins":{"added":1}}"#, "", None).await;
        assert_eq!(
            setup.prepare().await,
            UpdatePlan::Bump {
                from: "1".to_string(),
                to: "2".to_string()
            }
        );
        assert_eq!(setup.mirror_build(), "2");
        // Only the build moved: nothing was left unpacked.
        assert!(setup.sde_is_empty());

        set_sde_build(&setup.db(), "2").unwrap();
        let db = setup.db();
        let manager = SdeManager::new(&db, 1.0).unwrap();
        let (fingerprint, intact) = manager.get_fingerprint().unwrap().unwrap();
        assert!(intact);
        assert_eq!(fingerprint.sde_build.as_deref(), Some("2"));
        assert_eq!(
            setup.prepare().await,
            UpdatePlan::UpToDate {
                build: "2".to_string()
            }
        );
    }

    /// `tables` is spliced into the manifest right before `files`, so it can
    /// carry a `counts` member too.
    #[tokio::test]
    async fn published_record_counts_are_checked() {
        let matching = setup(
            "counts_ok",
            r#"{"skins":{"added":1}}, "counts": {"types": 1, "skins": 9}"#,
            "",
            None,
        )
        .await;
        assert!(matches!(matching.prepare().await, UpdatePlan::Bump { .. }));

        let wrong = setup(
            "counts_wrong",
            r#"{"skins":{"added":1}}, "counts": {"types": 5}"#,
            "",
            None,
        )
        .await;
        let plan = wrong.prepare().await;
        assert!(
            matches!(&plan, UpdatePlan::Full { reason: FullReason::Drift(drift) } if drift.len() == 1),
            "{plan:?}"
        );
        assert_eq!(wrong.mirror_build(), "1");
        assert!(wrong.sde_is_empty());
    }

    #[tokio::test]
    async fn a_change_to_unread_fields_only_bumps() {
        let delta = r#"{"table":"types","id":1,"op":"changed","fields":[{"path":"extra","old":1,"new":2}]}"#;
        let setup = setup("bump_field", r#"{"types":{"changed":1}}"#, delta, None).await;
        assert!(matches!(setup.prepare().await, UpdatePlan::Bump { .. }));
    }

    #[tokio::test]
    async fn a_change_to_read_fields_rebuilds_from_the_mirror() {
        let delta = r#"{"table":"types","id":1,"op":"changed","fields":[{"path":"name.en","old":"A","new":"B"}]}"#;
        let setup = setup("rebuild", r#"{"types":{"changed":1}}"#, delta, None).await;
        assert_eq!(
            setup.prepare().await,
            UpdatePlan::Rebuild {
                from: Some("1".to_string()),
                to: "2".to_string(),
                mirror_dir: setup.sde(),
            }
        );
        let types = std::fs::read_to_string(setup.sde().join("types.jsonl")).unwrap();
        assert!(types.contains("\"B\""), "{types}");
        // The database is still behind the committed mirror: rebuild again.
        assert!(matches!(setup.prepare().await, UpdatePlan::Rebuild { .. }));
        assert_eq!(setup.mirror_build(), "2");
    }

    #[tokio::test]
    async fn a_schema_change_to_read_fields_needs_a_full_build() {
        let delta = r#"{"table":"types","op":"schema","kind":"drop_path","path":"name.de"}"#;
        let setup = setup("schema", r#"{"types":{"changed":0}}"#, delta, None).await;
        let plan = setup.prepare().await;
        assert!(
            matches!(&plan, UpdatePlan::Full { reason: FullReason::SchemaChanged(alarms) } if alarms.len() == 1),
            "{plan:?}"
        );
        // Nothing was committed, and nothing is left unpacked.
        assert_eq!(setup.mirror_build(), "1");
        assert!(setup.sde_is_empty());
    }

    #[tokio::test]
    async fn a_corrupted_delta_needs_a_full_build() {
        let setup = setup("sha", r#"{"types":{"changed":1}}"#, "{}", Some("00")).await;
        assert!(matches!(
            setup.prepare().await,
            UpdatePlan::Full {
                reason: FullReason::Unavailable(_)
            }
        ));
    }

    #[tokio::test]
    async fn a_mirror_ahead_of_the_deltas_waits_for_them() {
        // A full build of build 5 while sde-deltas (and CCP's index here)
        // are still at 2: nothing to apply, nothing to download.
        let setup = setup("ahead", "{}", "", None).await;
        setup.edit_mirror(|meta| meta.build = "5".to_string());
        assert_eq!(
            setup.prepare().await,
            UpdatePlan::Rebuild {
                from: Some("1".to_string()),
                to: "5".to_string(),
                mirror_dir: setup.sde(),
            }
        );
        // The rebuild builds from the unpacked mirror; the caller releases it.
        assert!(setup.sde().join("types.jsonl").exists());
        release_working_copy(&setup.sde()).unwrap();
        set_sde_build(&setup.db(), "5").unwrap();
        assert_eq!(
            setup.prepare().await,
            UpdatePlan::UpToDate {
                build: "5".to_string()
            }
        );
        assert_eq!(setup.mirror_build(), "5");
        assert!(setup.sde_is_empty());
    }

    #[tokio::test]
    async fn a_current_database_leaves_the_mirror_packed() {
        // Up to date: nothing is unpacked to find that out, and `maps/` is
        // never touched.
        let setup = setup("packed", "{}", "", None).await;
        std::fs::create_dir_all(setup.sde().join("maps")).unwrap();
        std::fs::write(setup.sde().join("maps").join("a.svg"), "<svg/>").unwrap();
        set_sde_build(&setup.db(), "1").unwrap();
        // sde-deltas is at 2: the build is applied, nothing the parser reads.
        assert!(matches!(setup.prepare().await, UpdatePlan::Bump { .. }));
        set_sde_build(&setup.db(), "2").unwrap();
        assert!(matches!(setup.prepare().await, UpdatePlan::UpToDate { .. }));
        assert!(setup.sde_is_empty());
        assert!(setup.sde().join("maps").join("a.svg").exists());
    }

    #[tokio::test]
    async fn a_mirror_left_unpacked_by_0_6_is_packed_the_first_time() {
        let setup = setup("legacy", r#"{"skins":{"added":1}}"#, "", None).await;
        // Back to how 0.6.x left it: the mirror as a directory, no archive.
        Mirror::unpack(&setup.archive(), &setup.sde()).unwrap();
        std::fs::remove_file(setup.archive()).unwrap();

        assert!(matches!(setup.prepare().await, UpdatePlan::Bump { .. }));
        assert_eq!(setup.mirror_build(), "2");
        assert!(setup.sde_is_empty());
    }

    #[tokio::test]
    async fn a_failed_rebuild_is_retried_from_the_archive() {
        let delta = r#"{"table":"types","id":1,"op":"changed","fields":[{"path":"name.en","old":"A","new":"B"}]}"#;
        let setup = setup("retry", r#"{"types":{"changed":1}}"#, delta, None).await;
        assert!(matches!(setup.prepare().await, UpdatePlan::Rebuild { .. }));
        // The caller dies without releasing or building: the archive already
        // has build 2, and the next run starts from a clean unpack.
        std::fs::write(setup.sde().join("leftover.jsonl"), "{}").unwrap();
        assert_eq!(setup.mirror_build(), "2");
        let plan = setup.prepare().await;
        assert!(
            matches!(&plan, UpdatePlan::Rebuild { to, .. } if to == "2"),
            "{plan:?}"
        );
        assert!(!setup.sde().join("leftover.jsonl").exists());
        let types = std::fs::read_to_string(setup.sde().join("types.jsonl")).unwrap();
        assert!(types.contains("\"B\""), "{types}");
    }

    #[tokio::test]
    async fn a_database_from_another_parser_output_is_rebuilt_from_the_mirror() {
        let setup = setup("output", r#"{"skins":{"added":1}}"#, "", None).await;
        rusqlite::Connection::open(setup.db())
            .unwrap()
            .pragma_update(None, "user_version", PARSER_OUTPUT_VERSION + 1)
            .unwrap();
        assert_eq!(
            setup.prepare().await,
            UpdatePlan::Rebuild {
                from: None,
                to: "2".to_string(),
                mirror_dir: setup.sde(),
            }
        );
    }

    #[tokio::test]
    async fn no_mirror_or_no_chain_needs_a_full_build() {
        let setup = setup("coverage", "{}", "", None).await;
        setup.edit_mirror(|meta| meta.build = "0".to_string());
        assert_eq!(
            setup.prepare().await,
            UpdatePlan::Full {
                reason: FullReason::OutOfCoverage {
                    build: "0".to_string()
                }
            }
        );
        assert!(setup.sde_is_empty());
        std::fs::remove_file(setup.archive()).unwrap();
        assert_eq!(
            setup.prepare().await,
            UpdatePlan::Full {
                reason: FullReason::NoMirror
            }
        );
    }

    #[test]
    fn fingerprint_matches_compares_the_config_fields() {
        let config = ParserConfig::default();
        let fingerprint = SdeFingerprint {
            sde_build: Some("1".to_string()),
            language: config.language.clone(),
            position_2d: config.position_2d,
            map_kspace: config.map_kspace,
            map_wspace: config.map_wspace,
            map_abyssal: config.map_abyssal,
            map_void: config.map_void,
            with_gates: config.with_gates,
            with_moons: config.with_moons,
            with_third_party: config.with_third_party,
            with_icebelts: None,
            with_triglavian_status: None,
            with_jove_observatories: None,
            with_special_ore: None,
        };
        assert!(fingerprint_matches(&fingerprint, &config));
        let spanish = ParserConfig {
            language: "es".to_string(),
            ..config
        };
        assert!(!fingerprint_matches(&fingerprint, &spanish));
    }
}
