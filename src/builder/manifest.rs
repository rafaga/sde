//! HTTP fingerprint manifest for dotlan's maps.
//!
//! Equivalent to `_load_manifest` / `_save_manifest` / `_remote_map_fingerprint`
//! in the original `external_parser.py`: stores ETag/Last-Modified/Content-Length
//! per region so we don't re-download an SVG that hasn't changed on the
//! server.
//!
//! This module is deliberately network-agnostic: it only knows how to
//! read/write the manifest on disk and decide whether something changed,
//! given a remote fingerprint already fetched by the caller. That makes
//! it easy to test without mocking HTTP.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

/// HTTP fingerprint of a dotlan map at a given point in time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MapFingerprint {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub content_length: Option<String>,
}

/// The full manifest: region name -> last known fingerprint.
pub type Manifest = HashMap<String, MapFingerprint>;

/// Conventional path of the manifest inside `<sde_dir>/maps/_manifest.json`.
///
/// It deliberately lives inside `maps/`, the same folder that's
/// preserved when rebuilding `sde/` (see `builder::extract`), so it
/// isn't lost between runs.
pub fn manifest_path(maps_dir: &Path) -> PathBuf {
    maps_dir.join("_manifest.json")
}

/// Loads the manifest from disk. If it doesn't exist or is corrupted,
/// returns an empty manifest -- safe behavior: the next run simply
/// re-downloads everything, it never gets stuck because of this.
pub fn load(maps_dir: &Path) -> Manifest {
    profiling::function_scope!();

    let path = manifest_path(maps_dir);
    match std::fs::read_to_string(&path) {
        Ok(contents) => serde_json::from_str(&contents).unwrap_or_else(|err| {
            eprintln!(
                "http: manifest is not readable in {path:?} ({err}), rebuilding from scratch"
            );
            Manifest::new()
        }),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Manifest::new(),
        Err(err) => {
            eprintln!("http: can't read the manifest file in {path:?} ({err})");
            Manifest::new()
        }
    }
}

/// Saves the manifest to disk (creates `maps_dir` if needed).
pub fn save(maps_dir: &Path, manifest: &Manifest) -> io::Result<()> {
    profiling::function_scope!();

    std::fs::create_dir_all(maps_dir)?;
    let json = serde_json::to_string_pretty(manifest)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    std::fs::write(manifest_path(maps_dir), json)
}

/// Decides whether a region's map needs downloading, replicating the
/// logic of `process()` in the original `external_parser.py`:
///
/// - If the local file doesn't exist, it gets downloaded no matter what.
/// - If a remote fingerprint was obtained and it differs from the saved
///   one, it gets downloaded.
/// - If the remote fingerprint couldn't be obtained (no network, HEAD
///   failed, etc.) and the local file already exists, it's assumed
///   unchanged -- a one-off network hiccup doesn't block the build.
pub fn needs_download(
    local_file_exists: bool,
    cached: Option<&MapFingerprint>,
    remote: Option<&MapFingerprint>,
) -> bool {
    profiling::function_scope!();

    if !local_file_exists {
        return true;
    }
    match remote {
        Some(remote_fp) => Some(remote_fp) != cached,
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_fingerprint() -> MapFingerprint {
        MapFingerprint {
            etag: Some("\"abc123\"".to_string()),
            last_modified: Some("Wed, 15 Jan 2026 10:32:00 GMT".to_string()),
            content_length: Some("184320".to_string()),
        }
    }

    #[test]
    fn round_trip_save_and_load() {
        let dir = std::env::temp_dir().join(format!("sde-manifest-test-{}", std::process::id()));
        let maps_dir = dir.join("maps");

        let mut manifest = Manifest::new();
        manifest.insert("The Forge".to_string(), sample_fingerprint());

        save(&maps_dir, &manifest).expect("save should succeed");
        let loaded = load(&maps_dir);
        assert_eq!(loaded, manifest);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_missing_manifest_returns_empty() {
        let dir = std::env::temp_dir().join(format!("sde-manifest-missing-{}", std::process::id()));
        let manifest = load(&dir.join("maps"));
        assert!(manifest.is_empty());
    }

    #[test]
    fn needs_download_when_local_file_missing() {
        let fp = sample_fingerprint();
        assert!(needs_download(false, Some(&fp), Some(&fp)));
    }

    #[test]
    fn needs_download_when_fingerprint_changed() {
        let cached = sample_fingerprint();
        let mut remote = sample_fingerprint();
        remote.etag = Some("\"different\"".to_string());
        assert!(needs_download(true, Some(&cached), Some(&remote)));
    }

    #[test]
    fn no_download_when_fingerprint_unchanged() {
        let fp = sample_fingerprint();
        assert!(!needs_download(true, Some(&fp), Some(&fp)));
    }

    #[test]
    fn no_download_when_remote_check_failed_but_local_exists() {
        let cached = sample_fingerprint();
        assert!(!needs_download(true, Some(&cached), None));
    }

    #[test]
    fn download_when_no_cached_entry_yet() {
        let remote = sample_fingerprint();
        assert!(needs_download(true, None, Some(&remote)));
    }
}
