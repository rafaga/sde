//! Client for sde-deltas (<https://github.com/rafaga/sde-deltas>): the
//! build-to-build deltas of CCP's SDE, published as static files.
//!
//! The layout under the base URL ([`super::BuildUrls::deltas_url`]):
//!
//! - `index.json`: the chain of builds ([`DeltaIndex`]), each with the
//!   build it was made from.
//! - `<build>/manifest.json`: which tables the build changed and the
//!   SHA-256 of its files ([`DeltaManifest`]).
//! - `<build>/delta.jsonl.gz`: the delta itself, gzipped JSON Lines, applied
//!   by [`super::mirror::MirrorUpdate::apply`].
//!
//! Only format version [`FORMAT_VERSION`] is understood. Anything else is
//! an error, so the caller falls back to a full build.

use crate::Error;
use crate::builder::http;
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader};

/// The only sde-deltas format this module understands.
pub const FORMAT_VERSION: u64 = 1;

/// `index.json`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeltaIndex {
    pub format_version: u64,
    pub first_build: Option<u64>,
    pub latest_build: Option<u64>,
    pub builds: Vec<IndexEntry>,
}

/// One build in [`DeltaIndex`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexEntry {
    pub build: u64,
    /// The build the delta starts from.
    pub last_build: u64,
    pub release_date: String,
    /// `ok`, or `warn` when the data disagrees with CCP's own changelog
    /// (the data is still the source of truth).
    pub verification: String,
    #[serde(default)]
    pub delta_bytes: u64,
}

impl DeltaIndex {
    /// The builds that lead from `from` to [`Self::latest_build`], oldest
    /// first: empty when `from` already is the latest, `None` when the
    /// chain doesn't reach `from` (it's older than the first delta, or
    /// isn't a build in the chain at all).
    pub fn chain(&self, from: u64) -> Option<Vec<&IndexEntry>> {
        let by_build: HashMap<u64, &IndexEntry> = self
            .builds
            .iter()
            .map(|entry| (entry.build, entry))
            .collect();
        let mut chain = Vec::new();
        let mut current = self.latest_build?;
        while current != from {
            let entry = by_build.get(&current)?;
            chain.push(*entry);
            current = entry.last_build;
        }
        chain.reverse();
        Some(chain)
    }
}

/// `<build>/manifest.json` (the parts this crate uses).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeltaManifest {
    pub format_version: u64,
    pub build: u64,
    pub last_build: u64,
    /// Tables the build changed, by name.
    #[serde(default)]
    pub tables: BTreeMap<String, ManifestTable>,
    /// Published files, by name.
    pub files: BTreeMap<String, ManifestFile>,
    /// CCP's copyright notice, which has to travel with the data.
    #[serde(default)]
    pub notice: String,
    /// Records (distinct ids) of every table in the build, changed or not.
    /// `None` for builds published before sde-deltas added it.
    #[serde(default)]
    pub counts: Option<BTreeMap<String, u64>>,
}

/// One table in [`DeltaManifest::tables`].
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestTable {
    #[serde(default)]
    pub added: u64,
    #[serde(default)]
    pub removed: u64,
    #[serde(default)]
    pub changed: u64,
    /// The table's previous name, when it was renamed.
    pub renamed_from: Option<String>,
}

/// One file in [`DeltaManifest::files`].
#[derive(Debug, Clone, Deserialize)]
pub struct ManifestFile {
    pub bytes: u64,
    pub sha256: String,
}

impl DeltaManifest {
    /// Every table name the build touches, including a renamed table's old
    /// name.
    pub fn touched_tables(&self) -> impl Iterator<Item = &str> {
        self.tables.iter().flat_map(|(name, table)| {
            std::iter::once(name.as_str()).chain(table.renamed_from.as_deref())
        })
    }
}

/// Reads `<base_url>index.json`. `base_url` must end in `/`.
#[tracing::instrument]
pub async fn fetch_index(client: &Client, base_url: &str) -> Result<DeltaIndex, Error> {
    let text = http::fetch_text(client, &format!("{base_url}index.json")).await?;
    let index: DeltaIndex = serde_json::from_str(&text)?;
    check_format(index.format_version, "index.json")?;
    Ok(index)
}

/// Reads `<base_url><build>/manifest.json`.
#[tracing::instrument]
pub async fn fetch_manifest(
    client: &Client,
    base_url: &str,
    build: u64,
) -> Result<DeltaManifest, Error> {
    let text = http::fetch_text(client, &format!("{base_url}{build}/manifest.json")).await?;
    let manifest: DeltaManifest = serde_json::from_str(&text)?;
    check_format(manifest.format_version, "manifest.json")?;
    if manifest.build != build {
        return Err(Error::data(format!(
            "manifest of build {build} describes build {}",
            manifest.build
        )));
    }
    Ok(manifest)
}

/// Downloads the delta `manifest` describes, checks its SHA-256 and returns
/// its lines, in file order.
#[tracing::instrument(skip(manifest), fields(build = manifest.build))]
pub async fn fetch_delta(
    client: &Client,
    base_url: &str,
    manifest: &DeltaManifest,
) -> Result<Vec<Value>, Error> {
    const FILE: &str = "delta.jsonl.gz";
    let expected = manifest.files.get(FILE).ok_or_else(|| {
        Error::data(format!(
            "manifest of build {} lists no {FILE}",
            manifest.build
        ))
    })?;
    let url = format!("{base_url}{}/{FILE}", manifest.build);
    let compressed = http::fetch_bytes(client, &url).await?;
    let actual = sha256_hex(&compressed);
    if !actual.eq_ignore_ascii_case(&expected.sha256) {
        return Err(Error::data(format!(
            "{url}: SHA-256 is {actual}, the manifest says {}",
            expected.sha256
        )));
    }
    decode_delta(&compressed)
}

/// The lines of a gzipped JSON Lines delta.
pub fn decode_delta(compressed: &[u8]) -> Result<Vec<Value>, Error> {
    let reader = BufReader::new(flate2::read::GzDecoder::new(compressed));
    let mut lines = Vec::new();
    for line in reader.lines() {
        let line = line?;
        if !line.trim().is_empty() {
            lines.push(serde_json::from_str(&line)?);
        }
    }
    Ok(lines)
}

fn check_format(version: u64, file: &str) -> Result<(), Error> {
    if version == FORMAT_VERSION {
        Ok(())
    } else {
        Err(Error::data(format!(
            "{file} has sde-deltas format {version}, only {FORMAT_VERSION} is supported"
        )))
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::io::Write;

    fn entry(build: u64, last_build: u64) -> IndexEntry {
        IndexEntry {
            build,
            last_build,
            release_date: String::new(),
            verification: "ok".to_string(),
            delta_bytes: 0,
        }
    }

    fn index(entries: Vec<IndexEntry>) -> DeltaIndex {
        DeltaIndex {
            format_version: 1,
            first_build: entries.first().map(|entry| entry.build),
            latest_build: entries.last().map(|entry| entry.build),
            builds: entries,
        }
    }

    fn builds(chain: Option<Vec<&IndexEntry>>) -> Option<Vec<u64>> {
        chain.map(|chain| chain.iter().map(|entry| entry.build).collect())
    }

    #[test]
    fn chain_walks_from_a_build_to_the_latest() {
        let index = index(vec![entry(2, 1), entry(3, 2), entry(5, 3)]);
        assert_eq!(builds(index.chain(1)), Some(vec![2, 3, 5]));
        assert_eq!(builds(index.chain(3)), Some(vec![5]));
        assert_eq!(builds(index.chain(5)), Some(vec![]));
        // older than the first delta, or not a build of the chain
        assert_eq!(builds(index.chain(0)), None);
        assert_eq!(builds(index.chain(4)), None);
    }

    #[test]
    fn index_and_manifest_parse_the_published_shape() {
        let index: DeltaIndex = serde_json::from_str(
            r#"{"formatVersion": 1, "source": "x", "firstBuild": 2, "latestBuild": 2,
                "builds": [{"build": 2, "lastBuild": 1, "releaseDate": "2025-12-11T11:13:43Z",
                            "verification": "ok", "deltaBytes": 10}]}"#,
        )
        .unwrap();
        assert_eq!(
            index.builds,
            vec![IndexEntry {
                delta_bytes: 10,
                release_date: "2025-12-11T11:13:43Z".into(),
                ..entry(2, 1)
            }]
        );

        let manifest: DeltaManifest = serde_json::from_str(
            r#"{"formatVersion": 1, "build": 2, "lastBuild": 1, "notice": "(c) CCP",
                "tables": {"types": {"added": 1, "changed": 0, "removed": 0},
                           "mapMoons": {"added": 0, "changed": 1, "removed": 0, "renamedFrom": "moons"}},
                "files": {"delta.jsonl.gz": {"bytes": 3, "sha256": "ab"}}}"#,
        )
        .unwrap();
        let touched: Vec<&str> = manifest.touched_tables().collect();
        assert_eq!(touched, ["mapMoons", "moons", "types"]);
        assert_eq!(manifest.counts, None);

        let manifest: DeltaManifest = serde_json::from_str(
            r#"{"formatVersion": 1, "build": 2, "lastBuild": 1,
                "files": {}, "counts": {"types": 7, "mapMoons": 3}}"#,
        )
        .unwrap();
        assert_eq!(
            manifest.counts,
            Some(BTreeMap::from([
                ("mapMoons".to_string(), 3),
                ("types".to_string(), 7)
            ]))
        );
    }

    #[test]
    fn decode_delta_reads_gzipped_lines() {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder
            .write_all(b"{\"table\":\"t\",\"id\":1,\"op\":\"removed\"}\n\n{\"table\":\"t\",\"id\":2,\"op\":\"removed\"}\n")
            .unwrap();
        let lines = decode_delta(&encoder.finish().unwrap()).unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1]["id"], 2);
    }

    #[test]
    fn other_formats_are_rejected() {
        assert!(check_format(1, "x").is_ok());
        assert!(check_format(2, "x").is_err());
    }

    #[test]
    fn sha256_hex_matches_a_known_digest() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
