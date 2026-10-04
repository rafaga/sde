//! A local, projected copy of the SDE that delta updates keep current.
//!
//! After a full build, [`Mirror::create`] rewrites every table the parser
//! read with only the fields it read ([`FieldUsage`]) and removes every
//! other file of the export -- `maps/` (dotlan's SVGs, see
//! [`super::extract`]) is left alone. The result is a fraction of CCP's
//! export, and still a directory of ordinary `<table>.jsonl` files, so
//! [`super::parser::Parser`] reads it unchanged.
//!
//! ## Kept as a single archive
//!
//! Between runs the mirror isn't left as a directory: [`Mirror::pack`] stores
//! it as one zip (~20 MB instead of ~110 MB) and the SDE directory is emptied
//! except for `maps/`. [`Mirror::archive_meta`] reads what's needed to decide
//! whether it can be used without unpacking anything, and
//! [`Mirror::unpack`] makes a working copy in the SDE directory, which is
//! emptied again (`extract::clean_except_maps`) once it's no longer needed.
//! The archive is the only authoritative copy: a working copy left by a
//! crash is discarded the next time.
//!
//! [`Mirror::begin`] then applies sde-deltas' build-to-build deltas
//! (format 1, see <https://github.com/rafaga/sde-deltas>) on top of it:
//!
//! - A line for a table the parser doesn't read, or a field change outside
//!   the fields it reads, is dropped. If nothing is left after a whole
//!   chain of builds, `sde.db` can't have changed: only its recorded build
//!   needs to move ([`ApplyReport::relevant`] is `false`).
//! - A `schema` operation that renames or drops a field (or table) the
//!   parser reads raises a [`SchemaAlarm`]: the parser's mapping may need
//!   attention, and only a full build from CCP's export can tell.
//! - A field change whose `old` value doesn't match the mirror, an `added`
//!   record that already exists, a `removed` or `changed` one that doesn't,
//!   and other inconsistencies are drift ([`ApplyReport::drift`]): the
//!   mirror no longer matches the build it claims to be.
//!
//! In both of the last two cases the caller should fall back to a full
//! build, which creates a fresh mirror.
//!
//! ## Record model
//!
//! Changes are applied the way sde-deltas' own reference applier does: on
//! the record flattened to `path -> leaf` (`divisions.[3].size`), where a
//! leaf is a scalar, `null` or an empty list. Unlike sde-deltas' flattening,
//! an empty object is kept as a leaf too: the projection uses `{}` for a
//! list element none of whose fields are read, so the list keeps its
//! length (the parser may iterate it).

use crate::Error;
use crate::builder::parser::{PARSER_READS_VERSION, ParserConfig};
use crate::builder::usage::{FieldUsage, normalize_path};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::{BufRead, BufWriter, Write};
use std::path::{Path, PathBuf};

/// Version of the mirror's on-disk layout. A mirror with another one is
/// ignored ([`Mirror::open`] returns `None`).
pub const MIRROR_FORMAT: u32 = 1;

/// Name of the file that describes the mirror, inside its directory.
pub const META_FILE: &str = "_mirror.json";

/// Name of the file that holds the mirror's [`FieldUsage`].
pub const USAGE_FILE: &str = "_usage.json";

/// Everything [`Mirror`] keeps in [`META_FILE`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MirrorMeta {
    pub format: u32,
    /// SDE build the mirror's content matches.
    pub build: String,
    /// Version of this crate that recorded the field usage (informative).
    pub crate_version: String,
    /// [`PARSER_READS_VERSION`] of the parser that recorded the field usage:
    /// a parser with another one may read other fields. Mirrors written by
    /// 0.6.0, before this field existed, were all version 1.
    #[serde(default = "first_reads_version")]
    pub reads_version: u32,
    /// [`config_key`] of the config the field usage was recorded with.
    pub config: String,
    /// Builds applied from deltas since the mirror was created from a full
    /// export.
    pub delta_builds: u32,
    /// Set while [`MirrorUpdate::commit`] is replacing files: a mirror left
    /// like this by a crash is not trusted.
    #[serde(default)]
    pub pending: bool,
    /// Per table, the read lists (`divisions.[]`) where some element has
    /// fields the mirror doesn't keep. In those, a delta can't say whether
    /// an element appeared or went away, so any such change is drift.
    #[serde(default)]
    pub lossy_lists: BTreeMap<String, BTreeSet<String>>,
}

/// [`MirrorMeta::reads_version`] of a mirror written before it existed.
fn first_reads_version() -> u32 {
    1
}

impl MirrorMeta {
    /// Whether this crate, with `config`, would read the same fields the
    /// mirror was projected with -- otherwise it may lack some of them. It
    /// compares [`PARSER_READS_VERSION`], not the crate version: a release
    /// that doesn't change what the parser reads keeps the mirror usable.
    pub fn matches(&self, config: &ParserConfig) -> bool {
        self.reads_version == PARSER_READS_VERSION && self.config == config_key(config)
    }
}

/// A single file name: no directory separators, nor `.` / `..`.
fn is_plain_file_name(name: &str) -> bool {
    let mut components = Path::new(name).components();
    matches!(
        (components.next(), components.next()),
        (Some(std::path::Component::Normal(_)), None)
    )
}

/// The parts of a [`ParserConfig`] that change which fields (or tables) the
/// parser reads. The language isn't one of them: localized fields are
/// recorded and kept whole, in every language.
pub fn config_key(config: &ParserConfig) -> String {
    format!(
        "position2d={:?};kspace={};wspace={};abyssal={};void={};gates={};moons={}",
        config.position_2d,
        config.map_kspace,
        config.map_wspace,
        config.map_abyssal,
        config.map_void,
        config.with_gates,
        config.with_moons,
    )
}

/// A schema change in a delta that touches something the parser reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaAlarm {
    /// Build whose delta carries the change.
    pub build: String,
    pub table: String,
    /// `rename_table`, `rename_path` or `drop_path`.
    pub kind: String,
    /// What changed: the path (or `from -> to`).
    pub detail: String,
}

impl std::fmt::Display for SchemaAlarm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "build {}: {} on `{}` ({})",
            self.build, self.kind, self.table, self.detail
        )
    }
}

/// What applying one or more deltas found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApplyReport {
    /// Whether any line changed something the parser reads.
    pub relevant: bool,
    pub alarms: Vec<SchemaAlarm>,
    /// Inconsistencies between the deltas and the mirror.
    pub drift: Vec<String>,
}

impl ApplyReport {
    /// Whether the mirror can still be trusted (no alarm and no drift).
    pub fn is_clean(&self) -> bool {
        self.alarms.is_empty() && self.drift.is_empty()
    }
}

/// A projected copy of the SDE in a directory. See the module docs.
#[derive(Debug, Clone)]
pub struct Mirror {
    dir: PathBuf,
    meta: MirrorMeta,
    usage: FieldUsage,
}

impl Mirror {
    /// Turns the full SDE export in `dir` into a mirror in place: every
    /// table in `usage` is rewritten with only the fields it lists, and
    /// every other file or directory except `maps/` is removed.
    #[tracing::instrument(skip(usage))]
    pub fn create(
        dir: &Path,
        usage: &FieldUsage,
        build: &str,
        config: &ParserConfig,
    ) -> Result<Mirror, Error> {
        let empty = BTreeSet::new();
        let mut lossy_lists = BTreeMap::new();
        for table in usage.tables() {
            let paths = usage.paths(table).unwrap_or(&empty);
            let source = table_path(dir, table);
            let target = dir.join(format!("{table}.jsonl.tmp"));
            let mut lossy = BTreeSet::new();
            {
                let mut writer = BufWriter::new(std::fs::File::create(&target)?);
                for record in read_records(&source)? {
                    let projected = project(&record?, paths, &mut lossy);
                    serde_json::to_writer(&mut writer, &projected)?;
                    writer.write_all(b"\n")?;
                }
                writer.flush()?;
            }
            std::fs::rename(&target, &source)?;
            if !lossy.is_empty() {
                lossy_lists.insert(table.to_string(), lossy);
            }
        }

        let keep: BTreeSet<PathBuf> = usage
            .tables()
            .map(|table| table_path(dir, table))
            .chain([dir.join("maps")])
            .collect();
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if keep.contains(&path) {
                continue;
            }
            if path.is_dir() {
                std::fs::remove_dir_all(&path)?;
            } else {
                std::fs::remove_file(&path)?;
            }
        }

        let meta = MirrorMeta {
            format: MIRROR_FORMAT,
            build: build.to_string(),
            crate_version: env!("CARGO_PKG_VERSION").to_string(),
            reads_version: PARSER_READS_VERSION,
            config: config_key(config),
            delta_builds: 0,
            pending: false,
            lossy_lists,
        };
        write_json(&dir.join(USAGE_FILE), usage)?;
        write_json(&dir.join(META_FILE), &meta)?;
        Ok(Mirror {
            dir: dir.to_path_buf(),
            meta,
            usage: usage.clone(),
        })
    }

    /// The mirror in `dir`, or `None` if there isn't a usable one: no
    /// [`META_FILE`], an unreadable one, another [`MIRROR_FORMAT`], or one
    /// a crashed update left half-written.
    pub fn open(dir: &Path) -> Option<Mirror> {
        let meta: MirrorMeta = read_json(&dir.join(META_FILE))?;
        if meta.format != MIRROR_FORMAT || meta.pending {
            return None;
        }
        let usage: FieldUsage = read_json(&dir.join(USAGE_FILE))?;
        Some(Mirror {
            dir: dir.to_path_buf(),
            meta,
            usage,
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn meta(&self) -> &MirrorMeta {
        &self.meta
    }

    pub fn build(&self) -> &str {
        &self.meta.build
    }

    pub fn usage(&self) -> &FieldUsage {
        &self.usage
    }

    /// Whether this crate, with `config`, would read the same fields the
    /// mirror was projected with -- otherwise it may lack some of them. See
    /// [`MirrorMeta::matches`].
    pub fn matches(&self, config: &ParserConfig) -> bool {
        self.meta.matches(config)
    }

    /// Stores the mirror as a single zip at `archive` (written to a
    /// temporary file and moved over it, so a crash keeps the previous one):
    /// every file of its directory except `maps/`.
    #[tracing::instrument(skip(self))]
    pub fn pack(&self, archive: &Path) -> Result<(), Error> {
        if let Some(parent) = archive.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temporary = archive.with_extension("zip.tmp");
        {
            let mut writer =
                zip::ZipWriter::new(BufWriter::new(std::fs::File::create(&temporary)?));
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            let mut names: Vec<String> = std::fs::read_dir(&self.dir)?
                .filter_map(Result::ok)
                .filter(|entry| entry.path().is_file())
                .filter_map(|entry| entry.file_name().into_string().ok())
                .collect();
            names.sort();
            for name in names {
                writer.start_file(name.as_str(), options)?;
                let mut file = std::fs::File::open(self.dir.join(&name))?;
                std::io::copy(&mut file, &mut writer)?;
            }
            writer.finish()?.flush()?;
        }
        std::fs::rename(&temporary, archive)?;
        Ok(())
    }

    /// The [`MirrorMeta`] of the mirror stored in `archive`, reading only
    /// that entry: `None` if there's no usable one (no archive, not a
    /// zip, no [`META_FILE`], another [`MIRROR_FORMAT`] or one a crash left
    /// half-written).
    pub fn archive_meta(archive: &Path) -> Option<MirrorMeta> {
        let mut zip =
            zip::ZipArchive::new(std::io::BufReader::new(std::fs::File::open(archive).ok()?))
                .ok()?;
        let meta: MirrorMeta = serde_json::from_reader(zip.by_name(META_FILE).ok()?).ok()?;
        (meta.format == MIRROR_FORMAT && !meta.pending).then_some(meta)
    }

    /// Makes a working copy of the mirror stored in `archive` in `dir`: first
    /// empties `dir` (keeping `maps/`, see [`super::extract::clean_except_maps`]),
    /// then extracts every file of the archive into it. File names with a
    /// path are refused: the archive only holds a flat list of files.
    #[tracing::instrument]
    pub fn unpack(archive: &Path, dir: &Path) -> Result<Mirror, Error> {
        let mut zip = zip::ZipArchive::new(std::io::BufReader::new(std::fs::File::open(archive)?))?;
        for index in 0..zip.len() {
            let name = zip.by_index(index)?.name().to_string();
            if !is_plain_file_name(&name) {
                return Err(Error::data(format!(
                    "{archive:?} holds an entry with a path (`{name}`), a mirror is a flat list of files"
                )));
            }
        }
        std::fs::create_dir_all(dir)?;
        crate::builder::extract::clean_except_maps(dir)?;
        for index in 0..zip.len() {
            let mut entry = zip.by_index(index)?;
            if entry.is_dir() {
                continue;
            }
            let mut file = std::fs::File::create(dir.join(entry.name()))?;
            std::io::copy(&mut entry, &mut file)?;
        }
        Mirror::open(dir)
            .ok_or_else(|| Error::data(format!("{archive:?} doesn't hold a usable mirror")))
    }

    /// Starts applying deltas. Nothing on disk changes until
    /// [`MirrorUpdate::commit`].
    pub fn begin(&self) -> MirrorUpdate<'_> {
        MirrorUpdate {
            mirror: self,
            tables: HashMap::new(),
            report: ApplyReport::default(),
            build: self.meta.build.clone(),
            builds: 0,
            lossy_lists: self.meta.lossy_lists.clone(),
        }
    }
}

/// Deltas applied in memory on top of a [`Mirror`]. See [`Mirror::begin`].
pub struct MirrorUpdate<'m> {
    mirror: &'m Mirror,
    tables: HashMap<String, Table>,
    report: ApplyReport,
    build: String,
    builds: u32,
    lossy_lists: BTreeMap<String, BTreeSet<String>>,
}

impl MirrorUpdate<'_> {
    /// What the deltas applied so far found.
    pub fn report(&self) -> &ApplyReport {
        &self.report
    }

    /// Build the update has reached.
    pub fn build(&self) -> &str {
        &self.build
    }

    /// Applies the delta that turns the current build into `build`, one
    /// parsed line at a time, in file order.
    pub fn apply(
        &mut self,
        build: &str,
        lines: impl IntoIterator<Item = Value>,
    ) -> Result<(), Error> {
        for line in lines {
            self.apply_line(build, &line)?;
        }
        self.build = build.to_string();
        self.builds += 1;
        Ok(())
    }

    fn apply_line(&mut self, build: &str, line: &Value) -> Result<(), Error> {
        let mirror = self.mirror;
        let usage = &mirror.usage;
        let table = text(line, "table")?;
        let op = text(line, "op")?;
        if op == "schema" {
            self.check_schema(build, table, line)?;
            return Ok(());
        }
        if !usage.uses_table(table) {
            return Ok(());
        }
        let paths = usage.paths(table).cloned().unwrap_or_default();
        let id = line
            .get("id")
            .and_then(Key::of)
            .ok_or_else(|| Error::data(format!("delta line without a usable `id`: {line}")))?;
        let state = load_table(&mut self.tables, &mirror.dir, table)?;
        let lossy = self.lossy_lists.entry(table.to_string()).or_default();
        let report = &mut self.report;
        match op {
            "added" => {
                let mut record = line
                    .get("record")
                    .cloned()
                    .unwrap_or_else(|| Value::Object(Map::new()));
                if let Value::Object(map) = &mut record {
                    map.insert("_key".to_string(), id.to_value());
                }
                if state.index.contains_key(&id) {
                    report.drift.push(format!(
                        "{table} {id}: added in build {build}, already present"
                    ));
                    return Ok(());
                }
                state.insert(id, project(&record, &paths, lossy));
                report.relevant = true;
            }
            "removed" => {
                if state.remove(&id).is_none() {
                    report.drift.push(format!(
                        "{table} {id}: removed in build {build}, not present"
                    ));
                    return Ok(());
                }
                report.relevant = true;
            }
            "changed" => {
                let fields = line
                    .get("fields")
                    .and_then(Value::as_array)
                    .ok_or_else(|| {
                        Error::data(format!("`changed` line without `fields`: {line}"))
                    })?;
                let touched: Vec<&Value> = fields
                    .iter()
                    .filter(|field| {
                        let path = field.get("path").and_then(Value::as_str).unwrap_or("");
                        usage.touches(table, path)
                    })
                    .collect();
                for field in fields {
                    if !adds_or_removes(field) {
                        continue;
                    }
                    let path = field.get("path").and_then(Value::as_str).unwrap_or("");
                    // An element of a read list may be appearing or going
                    // away, and the mirror can't tell if it keeps only
                    // some of the element's fields.
                    let unknowable = if usage.touches(table, path) {
                        in_list(path, |element| lossy.contains(&normalize_path(element)))
                    } else {
                        in_list(path, |element| usage.touches(table, element))
                    };
                    if unknowable {
                        report.drift.push(format!(
                            "{table} {id}: build {build} adds or removes `{path}` inside a list \
                             whose elements the mirror only keeps in part"
                        ));
                    }
                }
                if touched.is_empty() {
                    return Ok(());
                }
                let Some(record) = state.get(&id) else {
                    report.drift.push(format!(
                        "{table} {id}: changed in build {build}, not present"
                    ));
                    return Ok(());
                };
                let mut flat = flatten(record);
                for field in touched {
                    let path = field.get("path").and_then(Value::as_str).unwrap_or("");
                    if flat.get(path) != field.get("old") {
                        report.drift.push(format!(
                            "{table} {id}: `{path}` in build {build} expected {:?}, mirror has {:?}",
                            field.get("old"),
                            flat.get(path)
                        ));
                        continue;
                    }
                    match field.get("new") {
                        Some(new) => flat.insert(path.to_string(), new.clone()),
                        None => flat.remove(path),
                    };
                }
                let updated = unflatten(&flat, &id);
                state.replace(&id, updated);
                report.relevant = true;
            }
            other => {
                return Err(Error::data(format!(
                    "unknown delta operation `{other}`: {line}"
                )));
            }
        }
        Ok(())
    }

    fn check_schema(&mut self, build: &str, table: &str, line: &Value) -> Result<(), Error> {
        let usage = &self.mirror.usage;
        let kind = text(line, "kind")?;
        let field = |name: &str| line.get(name).and_then(Value::as_str).unwrap_or("");
        let alarm = match kind {
            "rename_table" => {
                let from = field("from");
                (usage.uses_table(table) || usage.uses_table(from))
                    .then(|| format!("{from} -> {table}"))
            }
            "rename_path" => {
                let (from, to) = (field("from"), field("to"));
                (usage.touches(table, from) || usage.touches(table, to))
                    .then(|| format!("{from} -> {to}"))
            }
            "drop_path" => {
                let path = field("path");
                usage.touches(table, path).then(|| path.to_string())
            }
            // Informative: the values come with the records.
            "add_path" => None,
            other => Some(format!("unknown schema operation `{other}`")),
        };
        if let Some(detail) = alarm {
            self.report.alarms.push(SchemaAlarm {
                build: build.to_string(),
                table: table.to_string(),
                kind: kind.to_string(),
                detail,
            });
        }
        Ok(())
    }

    /// Compares the number of records of every table the mirror holds, as the
    /// deltas applied so far left it, with `counts` (the records of each
    /// table in the build the update reached, as sde-deltas publishes them).
    /// A difference is drift. A table missing from `counts` should have no
    /// records.
    #[tracing::instrument(skip(self, counts))]
    pub fn check_counts(&mut self, counts: &BTreeMap<String, u64>) -> Result<(), Error> {
        let mirror = self.mirror;
        for table in mirror.usage.tables() {
            let actual = match self.tables.get(table) {
                Some(loaded) => loaded.index.len() as u64,
                None => count_records(&table_path(&mirror.dir, table))?,
            };
            let expected = counts.get(table).copied().unwrap_or(0);
            if actual != expected {
                self.report.drift.push(format!(
                    "{table}: {actual} records in the mirror, build {} has {expected}",
                    self.build
                ));
            }
        }
        Ok(())
    }

    /// Writes the tables that changed and moves the mirror to the build the
    /// update reached. Refuses (with `Error::data`) when the report isn't
    /// clean: such a mirror must be replaced by a full build instead.
    #[tracing::instrument(skip(self))]
    pub fn commit(self) -> Result<Mirror, Error> {
        if !self.report.is_clean() {
            return Err(Error::data(
                "refusing to commit a mirror update with schema alarms or drift",
            ));
        }
        let dir = &self.mirror.dir;
        let mut meta = self.mirror.meta.clone();
        meta.pending = true;
        write_json(&dir.join(META_FILE), &meta)?;
        for (name, table) in &self.tables {
            if table.dirty {
                table.write(&table_path(dir, name))?;
            }
        }
        meta.pending = false;
        meta.build = self.build.clone();
        meta.delta_builds += self.builds;
        meta.lossy_lists = self
            .lossy_lists
            .into_iter()
            .filter(|(_, lists)| !lists.is_empty())
            .collect();
        write_json(&dir.join(META_FILE), &meta)?;
        Ok(Mirror {
            dir: dir.clone(),
            meta,
            usage: self.mirror.usage.clone(),
        })
    }
}

/// A record id: SDE `_key`s are integers, or strings (UUIDs) in a few
/// tables.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Key {
    Int(i64),
    Str(String),
}

impl Key {
    fn of(value: &Value) -> Option<Key> {
        match value {
            Value::Number(number) => number.as_i64().map(Key::Int),
            Value::String(text) => Some(Key::Str(text.clone())),
            _ => None,
        }
    }

    fn to_value(&self) -> Value {
        match self {
            Key::Int(number) => Value::from(*number),
            Key::Str(text) => Value::from(text.clone()),
        }
    }
}

impl std::fmt::Display for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Key::Int(number) => write!(f, "{number}"),
            Key::Str(text) => write!(f, "{text}"),
        }
    }
}

/// One table of the mirror, loaded whole.
struct Table {
    /// `None` for a removed record (so `index` stays valid).
    records: Vec<Option<Value>>,
    index: HashMap<Key, usize>,
    /// Whether the file was in `_key` order (CCP's are): new records then
    /// go in order too.
    sorted: bool,
    dirty: bool,
}

impl Table {
    fn load(path: &Path) -> Result<Table, Error> {
        let mut records = Vec::new();
        let mut index = HashMap::new();
        let mut sorted = true;
        let mut previous: Option<Key> = None;
        for record in read_records(path)? {
            let record = record?;
            let key = record.get("_key").and_then(Key::of).ok_or_else(|| {
                Error::data(format!(
                    "record without a usable `_key` in {path:?}: {record}"
                ))
            })?;
            if previous.as_ref().is_some_and(|previous| previous > &key) {
                sorted = false;
            }
            previous = Some(key.clone());
            index.insert(key, records.len());
            records.push(Some(record));
        }
        Ok(Table {
            records,
            index,
            sorted,
            dirty: false,
        })
    }

    fn get(&self, key: &Key) -> Option<&Value> {
        self.index
            .get(key)
            .and_then(|&position| self.records[position].as_ref())
    }

    fn insert(&mut self, key: Key, record: Value) {
        self.index.insert(key, self.records.len());
        self.records.push(Some(record));
        self.dirty = true;
    }

    fn remove(&mut self, key: &Key) -> Option<Value> {
        let position = self.index.remove(key)?;
        self.dirty = true;
        self.records[position].take()
    }

    fn replace(&mut self, key: &Key, record: Value) {
        if let Some(&position) = self.index.get(key) {
            self.records[position] = Some(record);
            self.dirty = true;
        }
    }

    fn write(&self, path: &Path) -> Result<(), Error> {
        let mut records: Vec<&Value> = self.records.iter().flatten().collect();
        if self.sorted {
            records.sort_by_cached_key(|record| record.get("_key").and_then(Key::of));
        }
        let temporary = path.with_extension("jsonl.tmp");
        {
            let mut writer = BufWriter::new(std::fs::File::create(&temporary)?);
            for record in records {
                serde_json::to_writer(&mut writer, record)?;
                writer.write_all(b"\n")?;
            }
            writer.flush()?;
        }
        std::fs::rename(&temporary, path)?;
        Ok(())
    }
}

/// The mirror's `table`, loaded on first use.
fn load_table<'t>(
    tables: &'t mut HashMap<String, Table>,
    dir: &Path,
    table: &str,
) -> Result<&'t mut Table, Error> {
    if !tables.contains_key(table) {
        let loaded = Table::load(&table_path(dir, table))?;
        tables.insert(table.to_string(), loaded);
    }
    Ok(tables.get_mut(table).expect("just inserted"))
}

/// Records (distinct `_key`s) in a table file, without keeping them.
fn count_records(path: &Path) -> Result<u64, Error> {
    let mut keys = std::collections::HashSet::new();
    for record in read_records(path)? {
        let record = record?;
        if let Some(key) = record.get("_key").and_then(Key::of) {
            keys.insert(key);
        }
    }
    Ok(keys.len() as u64)
}

/// Whether a `changed` entry adds a field (no `old`) or removes one (no
/// `new`), as opposed to changing its value.
fn adds_or_removes(field: &Value) -> bool {
    field.get("old").is_none() || field.get("new").is_none()
}

/// Whether `path` lies inside a list element (`divisions.[3]`, given with
/// its index) for which `matches` holds.
fn in_list(path: &str, matches: impl Fn(&str) -> bool) -> bool {
    let segments: Vec<&str> = path.split('.').collect();
    (0..segments.len())
        .filter(|&end| is_index(segments[end]))
        .any(|end| matches(&segments[..=end].join(".")))
}

fn is_index(segment: &str) -> bool {
    segment.starts_with('[') && segment.ends_with(']')
}

/// Whether a path (with list indices) is related to one of `paths` --
/// [`FieldUsage::touches`] for a single table's paths.
fn touches(paths: &BTreeSet<String>, path: &str) -> bool {
    let path = normalize_path(path);
    let changed: Vec<&str> = path.split('.').collect();
    paths.iter().any(|used| {
        let used: Vec<&str> = used.split('.').collect();
        let shared = used.len().min(changed.len());
        used[..shared] == changed[..shared]
    })
}

/// `record` with only the fields under (or above) `paths`, its `_key`, and
/// a `{}` for every element of a read list none of whose fields are kept.
/// Every read list where some element loses a field is added to `lossy`
/// (normalized, `divisions.[]`).
fn project(record: &Value, paths: &BTreeSet<String>, lossy: &mut BTreeSet<String>) -> Value {
    let flat = flatten(record);
    let mut kept = Flat::new();
    let mut elements = BTreeSet::new();
    for (path, value) in &flat {
        let keep = touches(paths, path);
        if keep {
            kept.insert(path.clone(), value.clone());
        }
        let segments: Vec<&str> = path.split('.').collect();
        for end in 0..segments.len() {
            if is_index(segments[end]) {
                let element = segments[..=end].join(".");
                if touches(paths, &element) {
                    if !keep {
                        lossy.insert(normalize_path(&element));
                    }
                    elements.insert(element);
                }
            }
        }
    }
    for element in elements {
        let below = format!("{element}.");
        let has_fields = kept.contains_key(&element)
            || kept
                .range(below.clone()..)
                .next()
                .is_some_and(|(path, _)| path.starts_with(&below));
        if !has_fields {
            kept.insert(element, Value::Object(Map::new()));
        }
    }
    let key = record.get("_key").and_then(Key::of);
    match key {
        Some(key) => unflatten(&kept, &key),
        None => unflatten_body(&kept),
    }
}

type Flat = BTreeMap<String, Value>;

/// `record` (without `_key`) as `path -> leaf`. See the module docs.
fn flatten(record: &Value) -> Flat {
    let mut flat = Flat::new();
    if let Value::Object(map) = record {
        for (name, value) in map {
            if name != "_key" {
                flatten_into(value, name.clone(), &mut flat);
            }
        }
    }
    flat
}

fn flatten_into(value: &Value, path: String, flat: &mut Flat) {
    match value {
        Value::Object(map) if !map.is_empty() => {
            for (name, child) in map {
                flatten_into(child, format!("{path}.{name}"), flat);
            }
        }
        Value::Array(items) if !items.is_empty() => {
            for (position, child) in items.iter().enumerate() {
                flatten_into(child, format!("{path}.[{position}]"), flat);
            }
        }
        _ => {
            flat.insert(path, value.clone());
        }
    }
}

enum Node {
    Leaf(Value),
    Object(BTreeMap<String, Node>),
    List(BTreeMap<usize, Node>),
}

impl Node {
    fn insert(&mut self, segments: &[&str], leaf: &Value) {
        let Some((first, rest)) = segments.split_first() else {
            // An empty container never replaces fields already below it.
            let empty = matches!(leaf, Value::Object(map) if map.is_empty())
                || matches!(leaf, Value::Array(items) if items.is_empty());
            if !(empty && !matches!(self, Node::Leaf(_))) {
                *self = Node::Leaf(leaf.clone());
            }
            return;
        };
        let index = first
            .strip_prefix('[')
            .and_then(|inner| inner.strip_suffix(']'))
            .and_then(|inner| inner.parse::<usize>().ok());
        match index {
            Some(position) => {
                if !matches!(self, Node::List(_)) {
                    *self = Node::List(BTreeMap::new());
                }
                if let Node::List(children) = self {
                    children
                        .entry(position)
                        .or_insert_with(|| Node::Leaf(Value::Null))
                        .insert(rest, leaf);
                }
            }
            None => {
                if !matches!(self, Node::Object(_)) {
                    *self = Node::Object(BTreeMap::new());
                }
                if let Node::Object(children) = self {
                    children
                        .entry((*first).to_string())
                        .or_insert_with(|| Node::Leaf(Value::Null))
                        .insert(rest, leaf);
                }
            }
        }
    }

    fn into_value(self) -> Value {
        match self {
            Node::Leaf(value) => value,
            Node::Object(children) => Value::Object(
                children
                    .into_iter()
                    .map(|(name, child)| (name, child.into_value()))
                    .collect(),
            ),
            Node::List(children) => {
                let length = children.keys().next_back().map_or(0, |last| last + 1);
                let mut items = vec![Value::Null; length];
                for (position, child) in children {
                    items[position] = child.into_value();
                }
                Value::Array(items)
            }
        }
    }
}

fn unflatten_body(flat: &Flat) -> Value {
    let mut root = Node::Object(BTreeMap::new());
    for (path, leaf) in flat {
        let segments: Vec<&str> = path.split('.').collect();
        root.insert(&segments, leaf);
    }
    root.into_value()
}

/// The record `flat` describes, with `_key` set to `key`.
fn unflatten(flat: &Flat, key: &Key) -> Value {
    let mut record = unflatten_body(flat);
    if let Value::Object(map) = &mut record {
        map.insert("_key".to_string(), key.to_value());
    }
    record
}

fn table_path(dir: &Path, table: &str) -> PathBuf {
    dir.join(format!("{table}.jsonl"))
}

fn read_records(path: &Path) -> Result<impl Iterator<Item = Result<Value, Error>>, Error> {
    let reader = std::io::BufReader::new(std::fs::File::open(path)?);
    Ok(reader.lines().filter_map(|line| match line {
        Ok(line) if line.trim().is_empty() => None,
        Ok(line) => Some(serde_json::from_str::<Value>(&line).map_err(Error::from)),
        Err(err) => Some(Err(Error::from(err))),
    }))
}

fn text<'a>(line: &'a Value, field: &str) -> Result<&'a str, Error> {
    line.get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| Error::data(format!("delta line without `{field}`: {line}")))
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    let contents = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&contents).ok()
}

/// Writes `value` as JSON to a temporary file and moves it over `path`.
fn write_json(path: &Path, value: &impl Serialize) -> Result<(), Error> {
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, serde_json::to_vec_pretty(value)?)?;
    std::fs::rename(&temporary, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::parser::Parser;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str, files: &[(&str, &str)]) -> Self {
            let path = std::env::temp_dir().join(format!(
                "sde_mirror_test_{name}_{}_{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::SeqCst)
            ));
            std::fs::create_dir_all(&path).unwrap();
            for (file, content) in files {
                std::fs::write(path.join(file), content).unwrap();
            }
            TempDir(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn usage(entries: &[(&str, &str)]) -> FieldUsage {
        let mut usage = FieldUsage::default();
        for (table, path) in entries {
            usage.insert(table, path);
        }
        usage
    }

    fn lines(dir: &Path, table: &str) -> Vec<Value> {
        read_records(&table_path(dir, table))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    /// A mirror of `t` (two records) reading `name`, `size` and the
    /// `divisions.[]` elements' `_key`/`size`.
    fn sample(name: &str) -> (TempDir, Mirror) {
        let dir = TempDir::new(
            name,
            &[
                (
                    "t.jsonl",
                    concat!(
                        "{\"_key\":1,\"name\":{\"en\":\"A\",\"de\":\"Ä\"},\"size\":1,\"extra\":5,",
                        "\"divisions\":[{\"_key\":7,\"size\":2}]}\n",
                        "{\"_key\":2,\"name\":{\"en\":\"B\"},\"size\":3,\"extra\":6}\n",
                    ),
                ),
                ("unused.jsonl", "{\"_key\":1}\n"),
                ("_sde.jsonl", "{\"_key\":\"sde\",\"buildNumber\":10}\n"),
            ],
        );
        std::fs::create_dir_all(dir.0.join("maps")).unwrap();
        std::fs::write(dir.0.join("maps").join("a.svg"), "<svg/>").unwrap();
        let usage = usage(&[
            ("t", "name"),
            ("t", "size"),
            ("t", "divisions.[]._key"),
            ("t", "divisions.[].size"),
        ]);
        let mirror = Mirror::create(&dir.0, &usage, "10", &ParserConfig::default()).unwrap();
        (dir, mirror)
    }

    #[test]
    fn flatten_and_unflatten_round_trip() {
        let record = json!({
            "_key": 3,
            "a": {"b": 1, "c": [1, 2]},
            "empty": [],
            "obj": {},
            "list": [{"x": 1}, {"x": 2, "y": null}]
        });
        let flat = flatten(&record);
        assert_eq!(flat.get("a.c.[1]"), Some(&json!(2)));
        assert_eq!(flat.get("empty"), Some(&json!([])));
        assert_eq!(flat.get("obj"), Some(&json!({})));
        assert_eq!(flat.get("list.[1].y"), Some(&Value::Null));
        assert_eq!(unflatten(&flat, &Key::Int(3)), record);
    }

    #[test]
    fn project_keeps_read_fields_and_list_lengths() {
        let paths = BTreeSet::from(["name".to_string(), "list.[].x".to_string()]);
        let mut lossy = BTreeSet::new();
        let record = json!({
            "_key": "u-1",
            "name": {"en": "A", "de": "B"},
            "other": 1,
            "list": [{"x": 1, "y": 2}, {"y": 3}]
        });
        let projected = project(&record, &paths, &mut lossy);
        assert_eq!(
            projected,
            json!({"_key": "u-1", "name": {"en": "A", "de": "B"}, "list": [{"x": 1}, {}]})
        );
        assert_eq!(lossy, BTreeSet::from(["list.[]".to_string()]));
    }

    #[test]
    fn create_projects_tables_and_removes_the_rest() {
        let (dir, mirror) = sample("create");
        assert_eq!(
            lines(&dir.0, "t"),
            [
                json!({"_key":1,"name":{"en":"A","de":"Ä"},"size":1,"divisions":[{"_key":7,"size":2}]}),
                json!({"_key":2,"name":{"en":"B"},"size":3}),
            ]
        );
        assert!(!dir.0.join("unused.jsonl").exists());
        assert!(!dir.0.join("_sde.jsonl").exists());
        assert!(dir.0.join("maps").join("a.svg").exists());
        assert_eq!(mirror.build(), "10");

        let reopened = Mirror::open(&dir.0).unwrap();
        assert_eq!(reopened.meta(), mirror.meta());
        assert_eq!(reopened.usage(), mirror.usage());
        assert!(reopened.matches(&ParserConfig::default()));
        let other = ParserConfig {
            with_moons: false,
            ..ParserConfig::default()
        };
        assert!(!reopened.matches(&other));
    }

    #[test]
    fn a_mirror_follows_what_the_parser_reads_not_the_crate_version() {
        let (dir, mirror) = sample("versions");
        let config = ParserConfig::default();

        // Another crate version that reads the same fields: still usable.
        let mut meta = mirror.meta().clone();
        meta.crate_version = "0.0.1".to_string();
        write_json(&dir.0.join(META_FILE), &meta).unwrap();
        assert!(Mirror::open(&dir.0).unwrap().matches(&config));

        // A parser that reads other fields: not usable.
        meta.reads_version = PARSER_READS_VERSION + 1;
        write_json(&dir.0.join(META_FILE), &meta).unwrap();
        assert!(!Mirror::open(&dir.0).unwrap().matches(&config));

        // Written by 0.6.0, before `readsVersion` existed: version 1.
        let mut old = serde_json::to_value(mirror.meta()).unwrap();
        old.as_object_mut().unwrap().remove("readsVersion");
        write_json(&dir.0.join(META_FILE), &old).unwrap();
        assert_eq!(Mirror::open(&dir.0).unwrap().meta().reads_version, 1);
    }

    #[test]
    fn changes_outside_read_fields_are_not_relevant() {
        let (_dir, mirror) = sample("irrelevant");
        let mut update = mirror.begin();
        update
            .apply(
                "11",
                [
                    json!({"table":"t","id":1,"op":"changed","fields":[{"path":"extra","old":5,"new":9}]}),
                    json!({"table":"unused","id":4,"op":"added","record":{"a":1}}),
                    json!({"table":"t","op":"schema","kind":"drop_path","path":"extra"}),
                ],
            )
            .unwrap();
        assert_eq!(update.report(), &ApplyReport::default());
        assert_eq!(update.build(), "11");
        let committed = update.commit().unwrap();
        assert_eq!(committed.build(), "11");
        assert_eq!(committed.meta().delta_builds, 1);
    }

    #[test]
    fn added_removed_and_changed_records_are_applied() {
        let (dir, mirror) = sample("apply");
        let mut update = mirror.begin();
        update
            .apply(
                "11",
                [
                    json!({"table":"t","id":0,"op":"added","record":{"name":{"en":"Z"},"size":0,"extra":1}}),
                    json!({"table":"t","id":2,"op":"removed"}),
                    json!({"table":"t","id":1,"op":"changed","fields":[
                        {"path":"name.de","old":"Ä","new":"Ae"},
                        {"path":"divisions.[1]._key","new":8},
                        {"path":"divisions.[1].size","new":4},
                        {"path":"extra","old":5,"new":7}
                    ]}),
                ],
            )
            .unwrap();
        assert!(update.report().relevant);
        assert!(update.report().is_clean(), "{:?}", update.report());
        update.commit().unwrap();
        assert_eq!(
            lines(&dir.0, "t"),
            [
                json!({"_key":0,"name":{"en":"Z"},"size":0}),
                json!({"_key":1,"name":{"en":"A","de":"Ae"},"size":1,
                       "divisions":[{"_key":7,"size":2},{"_key":8,"size":4}]}),
            ]
        );
        assert_eq!(Mirror::open(&dir.0).unwrap().build(), "11");
    }

    #[test]
    fn an_empty_list_can_grow_and_shrink() {
        let dir = TempDir::new(
            "lists",
            &[("t.jsonl", "{\"_key\":1,\"l\":[]}\n{\"_key\":2,\"l\":[7]}\n")],
        );
        let mirror =
            Mirror::create(&dir.0, &usage(&[("t", "l")]), "1", &ParserConfig::default()).unwrap();
        let mut update = mirror.begin();
        update
            .apply(
                "2",
                [
                    json!({"table":"t","id":1,"op":"changed","fields":[{"path":"l","old":[]},{"path":"l.[0]","new":7}]}),
                    json!({"table":"t","id":2,"op":"changed","fields":[{"path":"l","new":[]},{"path":"l.[0]","old":7}]}),
                ],
            )
            .unwrap();
        assert!(update.report().is_clean(), "{:?}", update.report());
        update.commit().unwrap();
        assert_eq!(
            lines(&dir.0, "t"),
            [json!({"_key":1,"l":[7]}), json!({"_key":2,"l":[]})]
        );
    }

    #[test]
    fn mismatches_are_drift_and_block_the_commit() {
        let (_dir, mirror) = sample("drift");
        let mut update = mirror.begin();
        update
            .apply(
                "11",
                [
                    json!({"table":"t","id":1,"op":"changed","fields":[{"path":"size","old":99,"new":2}]}),
                    json!({"table":"t","id":2,"op":"added","record":{"size":1}}),
                    json!({"table":"t","id":5,"op":"removed"}),
                ],
            )
            .unwrap();
        assert_eq!(update.report().drift.len(), 3, "{:?}", update.report());
        assert!(update.commit().is_err());
    }

    #[test]
    fn lists_kept_in_part_cannot_gain_or_lose_elements() {
        let dir = TempDir::new(
            "lossy",
            &[(
                "t.jsonl",
                "{\"_key\":1,\"l\":[{\"x\":1,\"y\":2},{\"x\":3,\"y\":4}]}\n",
            )],
        );
        let mirror = Mirror::create(
            &dir.0,
            &usage(&[("t", "l.[].x")]),
            "1",
            &ParserConfig::default(),
        )
        .unwrap();
        assert_eq!(
            mirror.meta().lossy_lists.get("t"),
            Some(&BTreeSet::from(["l.[]".to_string()]))
        );
        let mut update = mirror.begin();
        update
            .apply(
                "2",
                [json!({"table":"t","id":1,"op":"changed","fields":[
                    {"path":"l.[1].x","old":3},{"path":"l.[1].y","old":4}
                ]})],
            )
            .unwrap();
        assert_eq!(update.report().drift.len(), 2, "{:?}", update.report());

        // A value change inside the same list is fine.
        let mut update = mirror.begin();
        update
            .apply(
                "2",
                [json!({"table":"t","id":1,"op":"changed","fields":[{"path":"l.[1].x","old":3,"new":5}]})],
            )
            .unwrap();
        assert!(update.report().is_clean(), "{:?}", update.report());
    }

    #[test]
    fn schema_changes_to_read_fields_raise_alarms() {
        let (_dir, mirror) = sample("schema");
        let mut update = mirror.begin();
        update
            .apply(
                "11",
                [
                    json!({"table":"t","op":"schema","kind":"rename_path","from":"size","to":"volume"}),
                    json!({"table":"t","op":"schema","kind":"drop_path","path":"divisions.[].size"}),
                    json!({"table":"t","op":"schema","kind":"add_path","path":"size2"}),
                    json!({"table":"t","op":"schema","kind":"rename_path","from":"extra","to":"more"}),
                    json!({"table":"t2","op":"schema","kind":"rename_table","from":"t"}),
                ],
            )
            .unwrap();
        let kinds: Vec<&str> = update
            .report()
            .alarms
            .iter()
            .map(|alarm| alarm.kind.as_str())
            .collect();
        assert_eq!(kinds, ["rename_path", "drop_path", "rename_table"]);
        assert!(update.commit().is_err());
    }

    #[test]
    fn record_counts_are_checked_for_loaded_and_untouched_tables() {
        let dir = TempDir::new(
            "counts",
            &[
                ("t.jsonl", "{\"_key\":1,\"a\":1}\n{\"_key\":2,\"a\":2}\n"),
                ("u.jsonl", "{\"_key\":\"x\",\"b\":1}\n"),
            ],
        );
        let usage = usage(&[("t", "a"), ("u", "b")]);
        let mirror = Mirror::create(&dir.0, &usage, "1", &ParserConfig::default()).unwrap();

        // `t` is loaded by the delta, `u` is only counted from its file.
        let mut update = mirror.begin();
        update
            .apply(
                "2",
                [json!({"table":"t","id":3,"op":"added","record":{"a":3}})],
            )
            .unwrap();
        let counts = BTreeMap::from([("t".to_string(), 3), ("u".to_string(), 1)]);
        update.check_counts(&counts).unwrap();
        assert!(update.report().is_clean(), "{:?}", update.report());

        // A table missing from `counts` should be empty; a wrong count is drift.
        let mut update = mirror.begin();
        update
            .check_counts(&BTreeMap::from([("t".to_string(), 5)]))
            .unwrap();
        assert_eq!(update.report().drift.len(), 2, "{:?}", update.report());
        assert!(update.commit().is_err());
    }

    #[test]
    fn a_mirror_packs_into_one_zip_and_unpacks_into_a_clean_directory() {
        let (dir, mirror) = sample("pack");
        let archive = dir.0.join("out").join("mirror.zip");
        mirror.pack(&archive).unwrap();
        assert!(archive.is_file());
        assert!(!archive.with_extension("zip.tmp").exists());
        // Smaller than what it holds, and without `maps/`.
        let meta = Mirror::archive_meta(&archive).unwrap();
        assert_eq!(&meta, mirror.meta());
        assert!(meta.matches(&ParserConfig::default()));

        // Unpacks over leftovers (which go), keeping `maps/`.
        let work = TempDir::new("pack_work", &[("leftover.jsonl", "{}")]);
        std::fs::create_dir_all(work.0.join("maps")).unwrap();
        std::fs::write(work.0.join("maps").join("a.svg"), "<svg/>").unwrap();
        let unpacked = Mirror::unpack(&archive, &work.0).unwrap();
        assert_eq!(unpacked.meta(), mirror.meta());
        assert_eq!(unpacked.usage(), mirror.usage());
        assert!(!work.0.join("leftover.jsonl").exists());
        assert!(!work.0.join("maps").join("maps").exists());
        assert!(work.0.join("maps").join("a.svg").exists());
        assert_eq!(lines(&work.0, "t"), lines(&dir.0, "t"));
        assert!(
            !work
                .0
                .join("maps")
                .join("a.svg")
                .metadata()
                .unwrap()
                .is_dir()
        );
    }

    #[test]
    fn archive_meta_ignores_what_is_not_a_usable_mirror() {
        let (dir, mirror) = sample("archive_meta");
        assert!(Mirror::archive_meta(&dir.0.join("missing.zip")).is_none());
        std::fs::write(dir.0.join("garbage.zip"), "not a zip").unwrap();
        assert!(Mirror::archive_meta(&dir.0.join("garbage.zip")).is_none());

        // A mirror a crash left half-written, and one of another format.
        for (name, edit) in [("pending.zip", "pending"), ("format.zip", "format")] {
            let mut meta = mirror.meta().clone();
            match edit {
                "pending" => meta.pending = true,
                _ => meta.format = MIRROR_FORMAT + 1,
            }
            write_json(&dir.0.join(META_FILE), &meta).unwrap();
            let broken = Mirror {
                dir: dir.0.clone(),
                meta,
                usage: mirror.usage().clone(),
            };
            broken.pack(&dir.0.join(name)).unwrap();
            assert!(Mirror::archive_meta(&dir.0.join(name)).is_none(), "{name}");
        }
    }

    #[test]
    fn an_archive_with_paths_in_it_is_refused() {
        let dir = TempDir::new("slip", &[]);
        let archive = dir.0.join("evil.zip");
        let mut writer = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        for name in ["_mirror.json", "../escaped.txt"] {
            writer.start_file(name, options).unwrap();
            writer.write_all(b"{}").unwrap();
        }
        writer.finish().unwrap();
        let work = dir.0.join("work");
        assert!(Mirror::unpack(&archive, &work).is_err());
        assert!(!dir.0.join("escaped.txt").exists());
        assert!(!work.exists());
    }

    #[test]
    fn a_half_written_mirror_is_not_opened() {
        let (dir, mirror) = sample("pending");
        let mut meta = mirror.meta().clone();
        meta.pending = true;
        write_json(&dir.0.join(META_FILE), &meta).unwrap();
        assert!(Mirror::open(&dir.0).is_none());
    }

    fn real_config() -> ParserConfig {
        ParserConfig {
            language: "en".to_string(),
            position_2d: crate::objects::Position2DMode::Orthogonal(
                crate::objects::ProjectedAxis::Y,
            ),
            map_kspace: true,
            map_wspace: true,
            map_abyssal: true,
            map_void: true,
            with_gates: true,
            with_moons: true,
            verbose: false,
            with_third_party: false,
        }
    }

    fn build_db(path: &Path, sde_dir: &Path) -> Parser {
        let _ = std::fs::remove_file(path);
        let mut connection = rusqlite::Connection::open(path).unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        let parser = Parser::new(sde_dir, real_config());
        parser.parse_data(&mut connection).unwrap();
        parser
    }

    /// Rows of every table that differ between the two databases.
    fn differences(a: &Path, b: &Path) -> Vec<String> {
        let connection = rusqlite::Connection::open(a).unwrap();
        connection
            .execute("ATTACH DATABASE ?1 AS other", [b.to_string_lossy()])
            .unwrap();
        let mut out = Vec::new();
        for table in crate::builder::schema::table_names() {
            for (left, right) in [("main", "other"), ("other", "main")] {
                let count: i64 = connection
                    .query_row(
                        &format!(
                            "SELECT count(*) FROM (SELECT * FROM {left}.{table} \
                             EXCEPT SELECT * FROM {right}.{table})"
                        ),
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                if count > 0 {
                    out.push(format!("{table}: {count} rows only in {left}"));
                }
            }
        }
        out
    }

    /// Builds `sde.db` from a full, extracted SDE export and again from its
    /// mirror, and checks both are the same. Copies the tables the parser
    /// reads to a temporary directory first (the mirror is created in
    /// place). Run with
    /// `SDE_FULL_DIR=<extracted export> cargo test --features builder -- --ignored projection`.
    #[test]
    #[ignore = "needs a full SDE export in SDE_FULL_DIR"]
    fn projection_builds_the_same_database_as_the_full_export() {
        let source = PathBuf::from(std::env::var("SDE_FULL_DIR").expect("SDE_FULL_DIR"));
        let work = TempDir::new("real", &[]);
        let full_db = work.0.join("full.db");
        let parser = build_db(&full_db, &source);
        let usage = parser.field_usage().unwrap();
        println!("{}", serde_json::to_string_pretty(&usage).unwrap());

        let mirror_dir = work.0.join("sde");
        std::fs::create_dir_all(&mirror_dir).unwrap();
        for table in usage.tables() {
            std::fs::copy(table_path(&source, table), table_path(&mirror_dir, table)).unwrap();
        }
        let mirror = Mirror::create(&mirror_dir, &usage, "real", &real_config()).unwrap();
        println!("lossy lists: {:?}", mirror.meta().lossy_lists);
        let size: u64 = usage
            .tables()
            .map(|table| {
                std::fs::metadata(table_path(&mirror_dir, table))
                    .unwrap()
                    .len()
            })
            .sum();
        println!("mirror size: {size} bytes");

        let mirror_db = work.0.join("mirror.db");
        build_db(&mirror_db, &mirror_dir);
        let differences = differences(&full_db, &mirror_db);
        assert!(differences.is_empty(), "{differences:#?}");
    }
}
