//! Decompression of the SDE zip into the builder's working directory,
//! preserving `maps/` (dotlan's SVGs, which come from a different
//! source and shouldn't be lost nor re-downloaded on every build).
//!
//! `ZipArchive::extract()`, which already ships in the `zip` crate
//! itself (version 8.x), is used here -- no auxiliary crate like
//! `zip-extensions` is needed.

use crate::builder::BuilderError;
use std::path::Path;

/// Decompresses `zip_path` into `destination` (creates it if it doesn't
/// exist). Overwrites existing files. A corrupt/invalid zip
/// propagates as an `Err` (via `BuilderError::Zip`) instead of returning
/// `false` -- consistent with the rest of this module (`extract_map_data`
/// is the only function that distinguishes "recoverable failure, retry
/// the download" from "genuine error" with a `bool`, because there it's
/// part of an explicit retry flow; here there's no possible retry inside
/// this function, so a direct `Err` is clearer than a `bool` the caller
/// would have to interpret).
pub fn unzip(zip_path: &Path, destination: &Path) -> Result<(), BuilderError> {
    profiling::function_scope!();

    let file = std::fs::File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    archive.extract(destination)?;
    Ok(())
}

/// Empties `sde_dir` (if it already exists) while preserving
/// `<sde_dir>/maps/` as-is. Does nothing if `sde_dir` doesn't exist yet
/// -- there's nothing to clean up.
///
/// The `read_dir` entry and `maps_dir` are compared directly, without
/// canonicalizing either path: that's enough because both are built
/// from the same `sde_dir` the same way, and canonicalizing `maps_dir`
/// upfront would fail if that folder doesn't exist yet (a perfectly
/// valid case -- e.g. the first time the builder runs, before
/// `community::process()` has downloaded any map).
pub fn clean_except_maps(sde_dir: &Path) -> Result<(), BuilderError> {
    profiling::function_scope!();

    if !sde_dir.exists() {
        return Ok(());
    }
    let maps_dir = sde_dir.join("maps");
    for entry in std::fs::read_dir(sde_dir)? {
        let path = entry?.path();
        if path == maps_dir {
            continue;
        }
        if path.is_dir() {
            std::fs::remove_dir_all(&path)?;
        } else {
            std::fs::remove_file(&path)?;
        }
    }
    Ok(())
}

/// Cleans `sde_dir` while preserving `maps/` and decompresses
/// `zip_path` into it -- a direct composition of [`clean_except_maps`]
/// followed by [`unzip`], in that order.
pub fn prepare_sde_directory(zip_path: &Path, sde_dir: &Path) -> Result<(), BuilderError> {
    profiling::function_scope!();

    clean_except_maps(sde_dir)?;
    unzip(zip_path, sde_dir)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("sde-extract-test-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Creates a valid zip at `path` with the given
    /// `(name, content)` entries, including subfolders if the name
    /// carries a `/`. Uses `SimpleFileOptions` -- the type `zip` 8.6.0's
    /// own documentation recommends for the simple case (without
    /// specifying `FileOptions`'s generic parameter by hand).
    fn build_test_zip(path: &Path, entries: &[(&str, &str)]) {
        let file = std::fs::File::create(path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, content) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(content.as_bytes()).unwrap();
        }
        writer.finish().unwrap();
    }

    #[test]
    fn unzip_extracts_files_and_subdirectories() {
        let dir = temp_dir("basic");
        let zip_path = dir.join("test.zip");
        build_test_zip(
            &zip_path,
            &[
                ("types.jsonl", "{\"_key\": 1}\n"),
                ("universe/regions/The_Forge.jsonl", "{\"_key\": 10000002}\n"),
            ],
        );
        let destination = dir.join("out");

        unzip(&zip_path, &destination).unwrap();

        let types_content = std::fs::read_to_string(destination.join("types.jsonl")).unwrap();
        assert_eq!(types_content, "{\"_key\": 1}\n");
        let region_content =
            std::fs::read_to_string(destination.join("universe/regions/The_Forge.jsonl")).unwrap();
        assert_eq!(region_content, "{\"_key\": 10000002}\n");
    }

    #[test]
    fn unzip_errors_on_invalid_zip() {
        let dir = temp_dir("invalid");
        let bad_zip = dir.join("not_a_zip.zip");
        std::fs::write(&bad_zip, b"this is not a zip").unwrap();
        let destination = dir.join("out");

        let result = unzip(&bad_zip, &destination);
        assert!(matches!(result, Err(BuilderError::Zip(_))));
    }

    #[test]
    fn clean_except_maps_does_nothing_when_directory_missing() {
        let dir = temp_dir("missing");
        let sde_dir = dir.join("does_not_exist_yet");
        // must not fail even if sde_dir doesn't exist
        clean_except_maps(&sde_dir).unwrap();
    }

    #[test]
    fn clean_except_maps_preserves_maps_removes_the_rest() {
        let dir = temp_dir("preserve");
        let sde_dir = dir.join("sde");
        std::fs::create_dir_all(sde_dir.join("maps")).unwrap();
        std::fs::write(sde_dir.join("maps").join("The_Forge.svg"), "old svg").unwrap();
        std::fs::write(sde_dir.join("types.jsonl"), "old data").unwrap();
        std::fs::create_dir_all(sde_dir.join("universe")).unwrap();
        std::fs::write(
            sde_dir.join("universe").join("region.jsonl"),
            "more old data",
        )
        .unwrap();

        clean_except_maps(&sde_dir).unwrap();

        assert!(sde_dir.join("maps").exists(), "maps/ must survive");
        assert!(
            sde_dir.join("maps").join("The_Forge.svg").exists(),
            "maps/'s content must not be touched either"
        );
        assert!(
            !sde_dir.join("types.jsonl").exists(),
            "types.jsonl must be removed"
        );
        assert!(
            !sde_dir.join("universe").exists(),
            "universe/ must be removed entirely"
        );
    }

    #[test]
    fn prepare_sde_directory_cleans_then_extracts() {
        let dir = temp_dir("prepare");
        let sde_dir = dir.join("sde");
        std::fs::create_dir_all(sde_dir.join("maps")).unwrap();
        std::fs::write(sde_dir.join("maps").join("Domain.svg"), "preserved svg").unwrap();
        std::fs::write(sde_dir.join("old_data.jsonl"), "data from a previous build").unwrap();

        let zip_path = dir.join("new_sde.zip");
        build_test_zip(&zip_path, &[("types.jsonl", "{\"_key\": 1}\n")]);

        prepare_sde_directory(&zip_path, &sde_dir).unwrap();

        assert!(!sde_dir.join("old_data.jsonl").exists());
        assert!(sde_dir.join("maps").join("Domain.svg").exists());
        let types_content = std::fs::read_to_string(sde_dir.join("types.jsonl")).unwrap();
        assert_eq!(types_content, "{\"_key\": 1}\n");
    }
}
