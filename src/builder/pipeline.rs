//! The two things `sde-builder` does, as library functions: [`build`] a
//! database from CCP's export (the latest build or a specific one), and
//! [`update`] one (through sde-deltas when it can, from CCP's export when it
//! can't). The binary only turns its arguments into a [`Pipeline`] and calls
//! these; every decision lives here, so a library consumer gets the same
//! behavior.
//!
//! Both report what they do as [`Event`]s, through a callback, and how they
//! ended as an [`Outcome`].
//!
//! ```no_run
//! # async fn demo() -> Result<(), sde::Error> {
//! use sde::builder::pipeline::{self, Pipeline, Workspace};
//! use sde::builder::{BuildUrls, http};
//! use sde::builder::parser::ParserConfig;
//! use sde::builder::update::UpdateOptions;
//!
//! let pipeline = Pipeline {
//!     workspace: Workspace::new("sde.db".into(), "data".into(), "sde".into()),
//!     urls: BuildUrls::default(),
//!     parser: ParserConfig::default(),
//!     keep_source: false,
//!     updates: UpdateOptions::default(),
//! };
//! let client = http::build_client().map_err(sde::Error::from)?;
//! let outcome = pipeline::update(&client, &pipeline, |event| println!("{event}")).await?;
//! println!("{outcome}");
//! # Ok(())
//! # }
//! ```

use crate::Error;
use crate::builder::BuildUrls;
use crate::builder::parser::{Parser, ParserConfig};
use crate::builder::update::{
    self, DeltaProgress, FullReason, UpdateOptions, UpdatePlan, installed_build,
};
use crate::builder::{extract, schema, sde_index};
use reqwest::Client;
use std::path::{Path, PathBuf};

/// Name, inside the data directory, of the zip that holds the mirror.
pub const MIRROR_ARCHIVE: &str = "sde-mirror.zip";

/// Where everything lives on disk. Once a build ends, what's left is the
/// database, the mirror as one zip in `data_dir` ([`Workspace::mirror_archive`])
/// and dotlan's maps in `sde_dir/maps`: nothing downloaded from CCP, nor
/// decompressed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    /// The database (`sde.db`).
    pub db: PathBuf,
    /// Scratch space for CCP's zip and the file recording its build.
    pub data_dir: PathBuf,
    /// The SDE files the parser reads: CCP's export, or the mirror a full
    /// build leaves behind. Its `maps/` folder is kept between builds.
    pub sde_dir: PathBuf,
}

impl Workspace {
    pub fn new(db: PathBuf, data_dir: PathBuf, sde_dir: PathBuf) -> Self {
        Self {
            db,
            data_dir,
            sde_dir,
        }
    }

    /// Checks the paths are safe to build in. A build deletes everything in
    /// `sde_dir` except `maps/`, so it can't be somewhere the build itself
    /// or the user lives:
    ///
    /// - no path is empty, and `db` isn't an existing directory;
    /// - `db` and `data_dir` aren't inside `sde_dir` (they'd be deleted),
    ///   and `sde_dir` isn't, or doesn't contain, the current directory
    ///   (`--sde-dir .` would delete the project).
    ///
    /// Paths are compared as written (made absolute and with `.`/`..`
    /// resolved lexically): symbolic links aren't followed.
    pub fn validate(&self) -> Result<(), Error> {
        for (name, path) in [
            ("database", &self.db),
            ("data directory", &self.data_dir),
            ("SDE directory", &self.sde_dir),
        ] {
            if path.as_os_str().is_empty() {
                return Err(Error::data(format!("the {name} path is empty")));
            }
        }
        if self.db.is_dir() {
            return Err(Error::data(format!(
                "the database path {} is a directory",
                self.db.display()
            )));
        }
        let db = lexical_absolute(&self.db)?;
        let data = lexical_absolute(&self.data_dir)?;
        let sde = lexical_absolute(&self.sde_dir)?;
        if db.starts_with(&sde) {
            return Err(Error::data(format!(
                "the database {} is inside the SDE directory {}, which a build empties",
                self.db.display(),
                self.sde_dir.display()
            )));
        }
        if data.starts_with(&sde) {
            return Err(Error::data(format!(
                "the data directory {} is inside the SDE directory {}, which a build empties",
                self.data_dir.display(),
                self.sde_dir.display()
            )));
        }
        let current = lexical_absolute(Path::new("."))?;
        if current.starts_with(&sde) {
            return Err(Error::data(format!(
                "the SDE directory {} contains the current directory, which a build would empty",
                self.sde_dir.display()
            )));
        }
        Ok(())
    }

    /// The mirror of the SDE delta updates keep, as a single zip.
    pub fn mirror_archive(&self) -> PathBuf {
        self.data_dir.join(MIRROR_ARCHIVE)
    }

    fn zip(&self, variant: &str) -> PathBuf {
        self.data_dir.join(format!("sde-{variant}.zip"))
    }

    fn build_file(&self, variant: &str) -> PathBuf {
        self.data_dir.join(format!("sde-{variant}.build"))
    }
}

/// Everything [`build`] and [`update`] need.
#[derive(Debug, Clone)]
pub struct Pipeline {
    pub workspace: Workspace,
    pub urls: BuildUrls,
    /// How the database is built. Delta updates only apply to a database
    /// built with the same settings.
    pub parser: ParserConfig,
    /// After a full build, keep CCP's export (`sde_dir` and the zip) as it
    /// is, instead of reducing `sde_dir` to the mirror delta updates need
    /// (see [`crate::builder::mirror`]). The next update is then a full
    /// build again.
    pub keep_source: bool,
    /// How [`update`] uses sde-deltas.
    pub updates: UpdateOptions,
}

/// Something that happens while [`build`] or [`update`] run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Delta `progress.done + 1` of `progress.total` is being applied.
    ApplyingDelta(DeltaProgress),
    /// The database is being rebuilt from the local mirror, without CCP's
    /// export.
    RebuildingFromMirror { from: Option<String>, to: String },
    /// A build from CCP's export starts. `reason` is why deltas couldn't be
    /// used, or `None` when [`build`] was called (a full build is what was
    /// asked for).
    FullBuild { reason: Option<FullReason> },
    /// Getting CCP's export: downloading it (`build` is the one asked for,
    /// `None` for the latest), unless the zip kept from before is already
    /// the right one.
    Downloading { build: Option<String> },
    /// Decompressing the export.
    Extracting,
    /// Parsing the SDE files into the database.
    Building,
    /// The SDE was reduced to what delta updates need and stored as the
    /// single zip `archive` (`bytes` long); the SDE directory was emptied
    /// except for `maps/`.
    MirrorKept { archive: PathBuf, bytes: u64 },
    /// The mirror couldn't be made. The database is built and in place, but
    /// the next update is a full build again; CCP's zip is kept so that one
    /// doesn't download it again.
    MirrorFailed { error: String },
    /// What the build no longer needs was removed: CCP's zip and the
    /// decompressed export (all but the mirror), `freed` bytes in all.
    CleanedUp { freed: u64 },
}

impl std::fmt::Display for Event {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Event::ApplyingDelta(progress) => write!(
                f,
                "applying delta {}/{} (build {})",
                progress.done + 1,
                progress.total,
                progress.build
            ),
            Event::RebuildingFromMirror { from, to } => write!(
                f,
                "rebuilding the database from the local mirror ({} -> {to})",
                from.as_deref().unwrap_or("none")
            ),
            Event::FullBuild {
                reason: Some(reason),
            } => {
                write!(f, "building from CCP's export: {reason}")
            }
            Event::FullBuild { reason: None } => write!(f, "building from CCP's export"),
            Event::Downloading { build: Some(build) } => {
                write!(f, "getting CCP's export of build {build}")
            }
            Event::Downloading { build: None } => write!(f, "getting CCP's latest export"),
            Event::Extracting => write!(f, "decompressing the export"),
            Event::Building => write!(f, "building the database"),
            Event::MirrorKept { archive, bytes } => write!(
                f,
                "kept a mirror of the SDE in {} ({}) for delta updates",
                archive.display(),
                format_size(*bytes)
            ),
            Event::MirrorFailed { error } => write!(
                f,
                "couldn't keep a mirror of the SDE, the next update will be a full build: {error}"
            ),
            Event::CleanedUp { freed } => {
                write!(
                    f,
                    "removed the downloaded export, freed {}",
                    format_size(*freed)
                )
            }
        }
    }
}

/// How [`build`] or [`update`] ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The database was already up to date; `build` is its build, if known.
    UpToDate { build: Option<String> },
    /// The database moved from build `from` to `to`, which changes nothing
    /// it holds: only its recorded build was updated.
    Bumped { from: String, to: String },
    /// The database was rebuilt from the local mirror.
    RebuiltFromMirror { from: Option<String>, to: String },
    /// The database was built from CCP's export (`build`, if known).
    /// `mirror` tells whether a mirror was left for delta updates.
    FullBuild { build: Option<String>, mirror: bool },
}

impl std::fmt::Display for Outcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Outcome::UpToDate { build: Some(build) } => {
                write!(f, "already up to date (build {build}), nothing to do")
            }
            Outcome::UpToDate { build: None } => write!(f, "already up to date, nothing to do"),
            Outcome::Bumped { from, to } => write!(
                f,
                "build {from} -> {to} changes nothing the database holds, only its build was updated"
            ),
            Outcome::RebuiltFromMirror { from, to } => write!(
                f,
                "rebuilt from the local mirror ({} -> {to})",
                from.as_deref().unwrap_or("none")
            ),
            Outcome::FullBuild {
                build: Some(build), ..
            } => write!(f, "built from CCP's export of build {build}"),
            Outcome::FullBuild { build: None, .. } => write!(f, "built from CCP's export"),
        }
    }
}

/// Builds the database from CCP's export, whether or not it's already up to
/// date: the latest build (`sde_build` is `None`) or the one given.
/// `sde_build` is validated ([`sde_index::parse_build_number`]).
///
/// Downloads the export (the zip kept from a previous run is reused when
/// it's the right build), decompresses it into `sde_dir`, builds the
/// database next to its final path and moves it over once it passes SQLite's
/// integrity check -- a failure at any step keeps the database there was --
/// and then, unless `keep_source`, reduces `sde_dir` to a mirror and removes
/// the zip.
#[tracing::instrument(skip(client, on_event))]
pub async fn build(
    client: &Client,
    pipeline: &Pipeline,
    sde_build: Option<&str>,
    mut on_event: impl FnMut(Event),
) -> Result<Outcome, Error> {
    pipeline.workspace.validate()?;
    let sde_build = sde_build.map(sde_index::parse_build_number).transpose()?;
    full_build(client, pipeline, sde_build, None, &mut on_event).await
}

/// Brings the database up to CCP's latest build the cheapest way there is:
///
/// 1. through sde-deltas ([`update::prepare`]): nothing to do, only the
///    recorded build moves, or the database is rebuilt from the local
///    mirror -- no download of CCP's export in any of them;
/// 2. when deltas can't be used ([`FullReason`]), like [`build`] for the
///    latest build.
///
/// If the reason is that sde-deltas can't be reached or lags behind CCP,
/// but the database is already at CCP's latest build, there's nothing to do;
/// the same when CCP's side can't be read either.
#[tracing::instrument(skip(client, on_event))]
pub async fn update(
    client: &Client,
    pipeline: &Pipeline,
    mut on_event: impl FnMut(Event),
) -> Result<Outcome, Error> {
    let workspace = &pipeline.workspace;
    workspace.validate()?;
    let plan = update::prepare(
        client,
        &pipeline.urls,
        &workspace.db,
        &workspace.sde_dir,
        &workspace.mirror_archive(),
        &pipeline.parser,
        pipeline.updates,
        |progress| {
            // `prepare` also reports the finished chain (`done == total`).
            if progress.done < progress.total {
                on_event(Event::ApplyingDelta(progress));
            }
        },
    )
    .await;
    match plan {
        UpdatePlan::UpToDate { build } => Ok(Outcome::UpToDate { build: Some(build) }),
        UpdatePlan::Bump { from, to } => {
            update::set_sde_build(&workspace.db, &to)?;
            Ok(Outcome::Bumped { from, to })
        }
        UpdatePlan::Rebuild {
            from,
            to,
            mirror_dir,
        } => {
            on_event(Event::RebuildingFromMirror {
                from: from.clone(),
                to: to.clone(),
            });
            on_event(Event::Building);
            let built = build_database(client, pipeline, &mirror_dir, Some(&to)).await;
            // The unpacked mirror goes whether or not the build worked: the
            // archive keeps it.
            let released = update::release_working_copy(&mirror_dir);
            built?;
            released?;
            Ok(Outcome::RebuiltFromMirror { from, to })
        }
        UpdatePlan::Full { reason } => {
            if matches!(
                reason,
                FullReason::Unavailable(_) | FullReason::Lagging { .. }
            ) && let Some(installed) = installed_build(&workspace.db, &pipeline.parser)
            {
                let latest = sde_index::fetch_latest(client, &pipeline.urls.sde_url).await;
                if !matches!(&latest, Ok(Some(latest)) if latest.build != installed) {
                    return Ok(Outcome::UpToDate {
                        build: Some(installed),
                    });
                }
            }
            full_build(client, pipeline, None, Some(reason), &mut on_event).await
        }
    }
}

/// [`build`] without validating `sde_build`; `reason` is why [`update`]
/// fell back to it.
async fn full_build(
    client: &Client,
    pipeline: &Pipeline,
    sde_build: Option<String>,
    reason: Option<FullReason>,
    on_event: &mut impl FnMut(Event),
) -> Result<Outcome, Error> {
    let workspace = &pipeline.workspace;
    let variant = pipeline.urls.sde_variant.as_str();
    let zip = workspace.zip(variant);
    let build_file = workspace.build_file(variant);

    on_event(Event::FullBuild { reason });
    on_event(Event::Downloading {
        build: sde_build.clone(),
    });
    match &sde_build {
        Some(build) => {
            let kept = std::fs::read_to_string(&build_file)
                .is_ok_and(|kept| kept.trim() == build.as_str())
                && zip.exists();
            if !kept {
                sde_index::download_build(
                    client,
                    &workspace.data_dir,
                    &pipeline.urls.sde_url,
                    variant,
                    build,
                )
                .await?;
            }
        }
        None => {
            sde_index::update_as_needed(
                client,
                &workspace.data_dir,
                &pipeline.urls.sde_url,
                variant,
            )
            .await?;
        }
    }
    if !zip.exists() {
        // `update_as_needed` treats a network failure as "nothing new".
        return Err(Error::data(format!(
            "couldn't get CCP's export: {} doesn't exist (is {} reachable?)",
            zip.display(),
            pipeline.urls.sde_url
        )));
    }

    on_event(Event::Extracting);
    extract::prepare_sde_directory(&zip, &workspace.sde_dir)?;

    // The build of the zip just used: recorded in the database and the
    // mirror. A database with no recorded build (`sdeFingerprint.sdeBuild`
    // NULL) is still valid, so a missing file isn't an error.
    let build = std::fs::read_to_string(&build_file)
        .ok()
        .map(|build| build.trim().to_string())
        .filter(|build| !build.is_empty());

    on_event(Event::Building);
    let parser = build_database(client, pipeline, &workspace.sde_dir, build.as_deref()).await?;

    // The database is in place: what it was built from is only needed to
    // make the mirror delta updates use. Without one (no recorded build, or
    // `keep_source`) the export stays as it is.
    let mut mirror = false;
    if !pipeline.keep_source
        && let Some(build) = &build
    {
        let before = used_space(workspace, variant);
        let archive = workspace.mirror_archive();
        match update::create_mirror(&workspace.sde_dir, &archive, &parser, build) {
            Ok(_) => {
                mirror = true;
                on_event(Event::MirrorKept {
                    archive,
                    bytes: std::fs::metadata(workspace.mirror_archive()).map_or(0, |m| m.len()),
                });
                clean_downloads(workspace, variant)?;
                let freed = before.saturating_sub(used_space(workspace, variant));
                on_event(Event::CleanedUp { freed });
            }
            Err(error) => {
                // The database is fine; don't fail the build over this. A
                // half-reduced SDE directory is no use to anyone: empty
                // it, but keep the zip so the next build doesn't download
                // it again.
                on_event(Event::MirrorFailed {
                    error: error.to_string(),
                });
                extract::clean_except_maps(&workspace.sde_dir)?;
            }
        }
    }
    Ok(Outcome::FullBuild { build, mirror })
}

/// Removes what a build downloaded: CCP's zip (and the temporary file of an
/// interrupted download) and the file recording its build, and `data_dir`
/// itself when that leaves it empty. Returns the bytes freed. Whatever
/// isn't there is skipped. The mirror's archive and the decompressed export
/// are not touched: the export is reduced to the archive and emptied by
/// [`update::create_mirror`], or kept on purpose ([`Pipeline::keep_source`]).
///
/// [`build`] calls this once the database and its mirror are in place; it's
/// public for callers that drive the steps themselves.
pub fn clean_downloads(workspace: &Workspace, variant: &str) -> Result<u64, Error> {
    let mut freed = 0;
    for file in [
        workspace.zip(variant),
        workspace.data_dir.join(format!("sde-{variant}.zip.tmp")),
        workspace.build_file(variant),
    ] {
        match std::fs::metadata(&file) {
            Ok(metadata) => {
                std::fs::remove_file(&file)?;
                freed += metadata.len();
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    // Only goes if empty: `data_dir` may hold other things.
    let _ = std::fs::remove_dir(&workspace.data_dir);
    Ok(freed)
}

/// Bytes a build leaves on disk besides the database: CCP's zip, the SDE
/// directory and the mirror's archive (best effort: what can't be read
/// counts as zero).
fn used_space(workspace: &Workspace, variant: &str) -> u64 {
    fn tree(path: &Path) -> u64 {
        let Ok(metadata) = std::fs::symlink_metadata(path) else {
            return 0;
        };
        if !metadata.is_dir() {
            return metadata.len();
        }
        std::fs::read_dir(path)
            .map(|entries| entries.flatten().map(|entry| tree(&entry.path())).sum())
            .unwrap_or(0)
    }
    tree(&workspace.zip(variant))
        + tree(&workspace.data_dir.join(format!("sde-{variant}.zip.tmp")))
        + tree(&workspace.mirror_archive())
        + tree(&workspace.sde_dir)
}

/// A byte count for people: `48.2 MB`.
fn format_size(bytes: u64) -> String {
    let mb = bytes as f64 / (1024.0 * 1024.0);
    if mb >= 1.0 {
        format!("{mb:.1} MB")
    } else {
        format!("{:.0} KB", bytes as f64 / 1024.0)
    }
}

/// Builds the database from the SDE files in `sde_dir` (CCP's export or the
/// mirror), recording `build` in its fingerprint, and returns the parser that
/// read them (it knows which fields it read, see [`update::create_mirror`]).
///
/// Built next to the database and moved over it only once complete and
/// verified: a failed build keeps the database there was.
async fn build_database(
    client: &Client,
    pipeline: &Pipeline,
    sde_dir: &Path,
    build: Option<&str>,
) -> Result<Parser, Error> {
    let db = &pipeline.workspace.db;
    if let Some(parent) = db.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let building = building_path(db);
    if building.exists() {
        std::fs::remove_file(&building)?;
    }
    let parser = Parser::new(sde_dir, pipeline.parser.clone());
    let built = {
        let mut connection = rusqlite::Connection::open(&building)?;
        schema::create_schema(&connection)?;
        match parser
            .build_database(&mut connection, client, &pipeline.urls.maps_url, build)
            .await
        {
            Ok(_) => verify(&connection),
            Err(error) => Err(error),
        }
        // The connection closes here, before the file is moved or removed.
    };
    if let Err(error) = built {
        let _ = std::fs::remove_file(&building);
        return Err(error);
    }
    std::fs::rename(&building, db)?;
    Ok(parser)
}

/// `path` made absolute against the current directory, with `.` and `..`
/// resolved lexically (symbolic links aren't followed).
fn lexical_absolute(path: &Path) -> Result<PathBuf, Error> {
    use std::path::Component;
    let joined = std::env::current_dir()?.join(path);
    let mut resolved = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            other => resolved.push(other.as_os_str()),
        }
    }
    Ok(resolved)
}

/// SQLite's own quick integrity check of a database just built.
fn verify(connection: &rusqlite::Connection) -> Result<(), Error> {
    let result: String = connection.pragma_query_value(None, "quick_check", |row| row.get(0))?;
    if result == "ok" {
        return Ok(());
    }
    Err(Error::data(format!(
        "the new SDE database failed its integrity check: {result}"
    )))
}

/// Where a database is built before it replaces `db` (`sde.db` ->
/// `sde.db.building`).
fn building_path(db: &Path) -> PathBuf {
    let mut name = db.as_os_str().to_owned();
    name.push(".building");
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::mirror::Mirror;
    use std::io::Write;
    use wiremock::matchers::path;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// The tables [`Parser::parse_data`] reads, all of them needed to open.
    const TABLES: [&str; 17] = [
        "categories",
        "factions",
        "groups",
        "mapConstellations",
        "mapMoons",
        "mapPlanets",
        "mapRegions",
        "mapSolarSystems",
        "mapStargates",
        "mapStars",
        "npcCorporationDivisions",
        "npcCorporations",
        "npcStations",
        "races",
        "stationOperations",
        "stationServices",
        "types",
    ];

    /// Size of the table the export carries that the parser doesn't read.
    const UNUSED_BYTES: u64 = 50_000;

    /// A CCP export with every table the parser reads empty, and one it
    /// doesn't read.
    fn export() -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        for table in TABLES {
            writer
                .start_file(format!("{table}.jsonl"), options)
                .unwrap();
        }
        writer.start_file("unused.jsonl", options).unwrap();
        writer
            .write_all(&vec![b' '; UNUSED_BYTES as usize])
            .unwrap();
        writer.start_file("_sde.jsonl", options).unwrap();
        writer
            .write_all(b"{\"_key\":\"sde\",\"buildNumber\":100}\n")
            .unwrap();
        writer.finish().unwrap().into_inner()
    }

    struct Setup {
        dir: PathBuf,
        server: MockServer,
    }

    impl Drop for Setup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    impl Setup {
        fn pipeline(&self) -> Pipeline {
            Pipeline {
                workspace: Workspace::new(
                    self.dir.join("sde.db"),
                    self.dir.join("data"),
                    self.dir.join("sde"),
                ),
                urls: BuildUrls {
                    sde_url: format!("{}/ccp/", self.server.uri()),
                    deltas_url: format!("{}/deltas/", self.server.uri()),
                    maps_url: format!("{}/maps/", self.server.uri()),
                    ..BuildUrls::default()
                },
                parser: ParserConfig::default(),
                keep_source: false,
                updates: UpdateOptions::default(),
            }
        }

        async fn build(&self, sde_build: Option<&str>) -> (Result<Outcome, Error>, Vec<Event>) {
            let mut events = Vec::new();
            let outcome = build(&Client::new(), &self.pipeline(), sde_build, |event| {
                events.push(event);
            })
            .await;
            (outcome, events)
        }

        async fn update(&self) -> (Result<Outcome, Error>, Vec<Event>) {
            let mut events = Vec::new();
            let outcome =
                update(&Client::new(), &self.pipeline(), |event| events.push(event)).await;
            (outcome, events)
        }
    }

    /// A server with CCP's export of build 100 as its latest build, and
    /// sde-deltas at that build too.
    async fn setup(name: &str) -> Setup {
        static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "sde_pipeline_test_{name}_{}_{}",
            std::process::id(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        let server = MockServer::start().await;
        let routes: [(&str, Vec<u8>); 3] = [
            (
                "/ccp/latest.jsonl",
                b"{\"_key\":\"sde\",\"buildNumber\":100,\"releaseDate\":\"2999-01-01T00:00:00Z\"}"
                    .to_vec(),
            ),
            ("/ccp/eve-online-static-data-100-jsonl.zip", export()),
            (
                "/deltas/index.json",
                br#"{"formatVersion":1,"firstBuild":100,"latestBuild":100,"builds":[
                    {"build":100,"lastBuild":99,"releaseDate":"x","verification":"ok"}]}"#
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

    fn installed(setup: &Setup) -> Option<String> {
        installed_build(&setup.dir.join("sde.db"), &ParserConfig::default())
    }

    #[tokio::test]
    async fn a_specific_build_is_downloaded_built_and_mirrored() {
        let setup = setup("specific").await;
        // dotlan's maps, from an earlier run, are the one thing a build keeps.
        std::fs::create_dir_all(setup.dir.join("sde").join("maps")).unwrap();
        std::fs::write(setup.dir.join("sde").join("maps").join("a.svg"), "<svg/>").unwrap();
        let (outcome, events) = setup.build(Some("100")).await;
        assert_eq!(
            outcome.unwrap(),
            Outcome::FullBuild {
                build: Some("100".to_string()),
                mirror: true
            }
        );
        // The last event reports what was freed, net of the mirror's zip:
        // roughly the unused table the export carries.
        let Some(Event::CleanedUp { freed }) = events.last().cloned() else {
            panic!("no clean-up event: {events:?}");
        };
        assert!(freed > UNUSED_BYTES / 2, "freed only {freed} bytes");
        let archive = setup.dir.join("data").join("sde-mirror.zip");
        let bytes = std::fs::metadata(&archive).unwrap().len();
        assert_eq!(
            events[..events.len() - 1],
            [
                Event::FullBuild { reason: None },
                Event::Downloading {
                    build: Some("100".to_string())
                },
                Event::Extracting,
                Event::Building,
                Event::MirrorKept { archive, bytes },
            ]
        );
        assert_eq!(installed(&setup).as_deref(), Some("100"));
        assert!(!building_path(&setup.dir.join("sde.db")).exists());

        // On disk: the database, the mirror as one zip, and dotlan's maps.
        // Nothing downloaded or decompressed from CCP is left: not the zip,
        // the file recording its build, nor a single decompressed table.
        let names = |dir: &str| -> Vec<String> {
            let mut names: Vec<String> = std::fs::read_dir(setup.dir.join(dir))
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        };
        assert_eq!(names("data"), ["sde-mirror.zip"]);
        assert_eq!(names("sde"), ["maps"]);
        assert_eq!(names("sde/maps"), ["a.svg"]);
        assert!(setup.dir.join("sde.db").exists());
        assert_eq!(
            Mirror::archive_meta(&setup.dir.join("data").join("sde-mirror.zip"))
                .unwrap()
                .build,
            "100"
        );
    }

    #[test]
    fn clean_downloads_removes_the_zip_the_build_file_and_an_empty_data_directory() {
        let dir = std::env::temp_dir().join(format!("sde_pipeline_clean_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let workspace = Workspace::new(dir.join("sde.db"), dir.join("data"), dir.join("sde"));
        // Nothing there: nothing to do.
        assert_eq!(clean_downloads(&workspace, "jsonl").unwrap(), 0);

        std::fs::create_dir_all(&workspace.data_dir).unwrap();
        std::fs::write(workspace.data_dir.join("sde-jsonl.zip"), [0u8; 100]).unwrap();
        std::fs::write(workspace.data_dir.join("sde-jsonl.zip.tmp"), [0u8; 10]).unwrap();
        std::fs::write(workspace.data_dir.join("sde-jsonl.build"), "42").unwrap();
        assert_eq!(clean_downloads(&workspace, "jsonl").unwrap(), 112);
        assert!(!workspace.data_dir.exists());

        // Something else in `data/` stays, and so does the directory.
        std::fs::create_dir_all(&workspace.data_dir).unwrap();
        std::fs::write(workspace.data_dir.join("sde-jsonl.zip"), [0u8; 5]).unwrap();
        std::fs::write(workspace.data_dir.join("notes.txt"), "mine").unwrap();
        assert_eq!(clean_downloads(&workspace, "jsonl").unwrap(), 5);
        assert!(workspace.data_dir.join("notes.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn the_latest_build_is_built_when_none_is_given_and_keep_source_keeps_the_export() {
        let setup = setup("latest").await;
        let mut pipeline = setup.pipeline();
        pipeline.keep_source = true;
        let outcome = build(&Client::new(), &pipeline, None, |_| {})
            .await
            .unwrap();
        assert_eq!(
            outcome,
            Outcome::FullBuild {
                build: Some("100".to_string()),
                mirror: false
            }
        );
        assert!(setup.dir.join("data").join("sde-jsonl.zip").exists());
        assert!(setup.dir.join("sde").join("types.jsonl").exists());
        assert!(!setup.dir.join("sde").join("_mirror.json").exists());
    }

    #[tokio::test]
    async fn an_invalid_or_missing_build_changes_nothing() {
        let setup = setup("invalid").await;
        for text in ["abc", "0123", "-5", "12 34", "", "99999999999999999999999"] {
            let (outcome, events) = setup.build(Some(text)).await;
            assert!(outcome.is_err(), "{text:?} was accepted");
            assert!(events.is_empty());
        }
        // CCP doesn't have build 7 (404): the database isn't touched.
        let (outcome, _) = setup.build(Some("7")).await;
        assert!(outcome.is_err());
        assert!(!setup.dir.join("sde.db").exists());
        assert!(!building_path(&setup.dir.join("sde.db")).exists());
    }

    #[tokio::test]
    async fn update_builds_from_scratch_and_is_then_up_to_date() {
        let setup = setup("update").await;
        let (outcome, events) = setup.update().await;
        assert_eq!(
            outcome.unwrap(),
            Outcome::FullBuild {
                build: Some("100".to_string()),
                mirror: true
            }
        );
        assert_eq!(
            events.first(),
            Some(&Event::FullBuild {
                reason: Some(FullReason::NoMirror)
            })
        );
        assert_eq!(installed(&setup).as_deref(), Some("100"));

        let (outcome, events) = setup.update().await;
        assert_eq!(
            outcome.unwrap(),
            Outcome::UpToDate {
                build: Some("100".to_string())
            }
        );
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn update_without_deltas_keeps_a_current_database() {
        let setup = setup("no_deltas").await;
        setup.build(Some("100")).await.0.unwrap();

        // sde-deltas is gone, but the database is at CCP's latest build.
        let mut pipeline = setup.pipeline();
        pipeline.urls.deltas_url = format!("{}/missing/", setup.server.uri());
        let outcome = update(&Client::new(), &pipeline, |_| {}).await.unwrap();
        assert_eq!(
            outcome,
            Outcome::UpToDate {
                build: Some("100".to_string())
            }
        );
    }

    #[tokio::test]
    async fn update_rebuilds_from_the_mirror_when_the_database_is_missing() {
        let setup = setup("rebuild").await;
        setup.build(Some("100")).await.0.unwrap();
        std::fs::remove_file(setup.dir.join("sde.db")).unwrap();

        let (outcome, events) = setup.update().await;
        assert_eq!(
            outcome.unwrap(),
            Outcome::RebuiltFromMirror {
                from: None,
                to: "100".to_string()
            }
        );
        assert_eq!(
            events,
            [
                Event::RebuildingFromMirror {
                    from: None,
                    to: "100".to_string()
                },
                Event::Building,
            ]
        );
        assert_eq!(installed(&setup).as_deref(), Some("100"));
        // The mirror unpacked for the rebuild is gone again: only the maps
        // directory may remain in the SDE directory.
        assert!(
            std::fs::read_dir(setup.dir.join("sde"))
                .unwrap()
                .all(|entry| entry.unwrap().file_name() == "maps")
        );
        assert!(setup.dir.join("data").join("sde-mirror.zip").exists());
    }

    #[test]
    fn unsafe_paths_are_rejected_before_anything_is_deleted() {
        let workspace = |db: &str, data: &str, sde: &str| {
            Workspace::new(PathBuf::from(db), PathBuf::from(data), PathBuf::from(sde))
        };
        assert!(workspace("sde.db", "data", "sde").validate().is_ok());
        assert!(
            workspace("out/sde.db", "out/data", "out/sde")
                .validate()
                .is_ok()
        );
        // The SDE directory is emptied by a build.
        assert!(workspace("sde/sde.db", "data", "sde").validate().is_err());
        assert!(workspace("sde.db", "sde/data", "sde").validate().is_err());
        assert!(
            workspace("x/../sde/sde.db", "data", "sde")
                .validate()
                .is_err()
        );
        assert!(workspace("sde.db", "data", ".").validate().is_err());
        assert!(workspace("sde.db", "data", "..").validate().is_err());
        assert!(workspace("sde.db", "data", "sde/..").validate().is_err());
        // Empty paths, and a directory where the database goes.
        assert!(workspace("", "data", "sde").validate().is_err());
        assert!(workspace("sde.db", " ".trim(), "sde").validate().is_err());
        let dir = std::env::temp_dir();
        assert!(
            Workspace::new(dir, "data".into(), "sde".into())
                .validate()
                .is_err()
        );
    }

    #[tokio::test]
    async fn build_and_update_validate_the_workspace() {
        let setup = setup("validate").await;
        let mut pipeline = setup.pipeline();
        pipeline.workspace.sde_dir = PathBuf::from(".");
        assert!(
            build(&Client::new(), &pipeline, None, |_| {})
                .await
                .is_err()
        );
        assert!(update(&Client::new(), &pipeline, |_| {}).await.is_err());
        assert!(!setup.dir.join("data").exists());
    }

    #[test]
    fn events_and_outcomes_read_as_sentences() {
        let event = Event::FullBuild {
            reason: Some(FullReason::NoMirror),
        };
        assert_eq!(
            event.to_string(),
            "building from CCP's export: no local mirror of the SDE yet"
        );
        assert_eq!(
            Outcome::Bumped {
                from: "1".to_string(),
                to: "2".to_string()
            }
            .to_string(),
            "build 1 -> 2 changes nothing the database holds, only its build was updated"
        );
    }
}
