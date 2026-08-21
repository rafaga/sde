//! Checking the most recent SDE build number published by CCP
//! (`developers.eveonline.com`), and conditionally downloading the
//! corresponding zip.
//!
//! Just like [`super::http`], this module doesn't write anything to the
//! database -- it only handles files on disk (`latest.jsonl`, the
//! `.build` file with the locally saved number, and the zip itself).
//! Moving the downloaded `.zip` into the builder's working tree
//! (preserving `maps/`, see [`super::manifest::manifest_path`]) is
//! `builder::extract`'s job.

use crate::Error;
use crate::builder::http;
use reqwest::Client;
use std::path::Path;

/// Looks, line by line, for the `latest.jsonl` record with
/// `_key == "sde"` and returns its `buildNumber` as text.
///
/// Verified against a real `latest.jsonl` (August 2026): it carries a
/// single line (`{"_key": "sde", "buildNumber": 3458726, "releaseDate":
/// "..."}`, with a `\r\n` line ending) -- not several, one per dataset,
/// as was speculated before having a real sample. `buildNumber` comes in
/// as a JSON number (not as quoted text), and `str::lines()` (used
/// here) handles the `\r\n` correctly without leaving a stray `\r` that
/// would break JSON parsing -- confirmed line by line against the real
/// file, byte for byte.
///
/// The parsing remains deliberately tolerant beyond this confirmed case:
/// any line that isn't valid JSON, or that's missing the expected field,
/// is simply skipped instead of aborting the whole file -- in case it
/// ever carries more than one line, or the format changes.
///
/// `buildNumber` is returned as a `String` regardless of whether it was
/// a number or already text in the original JSON, since the build
/// number is an opaque identifier meant to be compared as text, not
/// something meant to be operated on numerically.
#[tracing::instrument]
fn find_sde_build_number(jsonl: &str) -> Option<String> {
    for line in jsonl.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(record) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if record.get("_key").and_then(|v| v.as_str()) != Some("sde") {
            continue;
        }
        let build = record.get("buildNumber")?;
        return Some(match build {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        });
    }
    None
}

/// Checks the most recent SDE build number
/// (`{sde_url_base}latest.jsonl`) and downloads
/// `eve-online-static-data-{build}-{variant}.zip` to
/// `<data_dir>/sde-{variant}.zip` only if it's newer than the one saved
/// locally in `<data_dir>/sde-{variant}.build`.
///
/// `sde_url_base` must end in `/` (e.g.
/// `"https://developers.eveonline.com/static-data/tranquility/"`).
/// `variant` is `"jsonl"` or `"yaml"` -- used as-is in file names,
/// without validating against a closed enum: if CCP adds a third export
/// format, this function needs no changes.
///
/// Returns `Ok(true)` if a new version was downloaded, `Ok(false)` if it
/// was already up to date -- or if the remote build couldn't be
/// determined (no network, `latest.jsonl` missing the expected record,
/// etc.): a one-off problem checking the version doesn't block the
/// whole build.
///
/// # Downloads to temp, then renames
///
/// This function downloads to a temporary file
/// (`sde-{variant}.zip.tmp`) and only replaces `sde-{variant}.zip` (via
/// `rename`, atomic on the same filesystem) once the download finished
/// successfully -- if it fails, the previous zip stays intact instead
/// of being deleted upfront and left missing.
#[tracing::instrument]
pub async fn update_as_needed(
    client: &Client,
    data_dir: &Path,
    sde_url_base: &str,
    variant: &str,
) -> Result<bool, Error> {
    std::fs::create_dir_all(data_dir)?;

    let build_file = data_dir.join(format!("sde-{variant}.build"));
    let zip_file = data_dir.join(format!("sde-{variant}.zip"));
    let temp_zip_file = data_dir.join(format!("sde-{variant}.zip.tmp"));

    let index_url = format!("{sde_url_base}latest.jsonl");
    let index_contents = match http::fetch_text(client, &index_url).await {
        Ok(contents) => contents,
        Err(err) => {
            eprintln!("sde_index: couldn't download {index_url} ({err})");
            return Ok(false);
        }
    };

    let Some(latest_build) = find_sde_build_number(&index_contents) else {
        eprintln!("sde_index: couldn't determine the most recent build number in {index_url}");
        return Ok(false);
    };

    let current_build = std::fs::read_to_string(&build_file)
        .ok()
        .map(|s| s.trim().to_string());

    if current_build.as_deref() == Some(latest_build.as_str()) && zip_file.exists() {
        println!("sde_index: {variant} data already up to date (build {latest_build})");
        return Ok(false);
    }

    println!(
        "sde_index: new build available ({} -> {latest_build}), downloading {variant} data",
        current_build.as_deref().unwrap_or("none")
    );

    let zip_url = format!("{sde_url_base}eve-online-static-data-{latest_build}-{variant}.zip");
    http::download(client, &zip_url, &temp_zip_file, |_| {}).await?;
    std::fs::rename(&temp_zip_file, &zip_file)?;
    std::fs::write(&build_file, &latest_build)?;

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_sde_build_number_extracts_matching_key() {
        let jsonl = "{\"_key\": \"sde\", \"buildNumber\": 12345}\n";
        assert_eq!(find_sde_build_number(jsonl), Some("12345".to_string()));
    }

    #[test]
    fn find_sde_build_number_ignores_other_keys() {
        let jsonl = concat!(
            "{\"_key\": \"universe\", \"buildNumber\": 99999}\n",
            "{\"_key\": \"sde\", \"buildNumber\": 12345}\n",
            "{\"_key\": \"bsd\", \"buildNumber\": 11111}\n",
        );
        assert_eq!(find_sde_build_number(jsonl), Some("12345".to_string()));
    }

    #[test]
    fn find_sde_build_number_returns_none_when_missing() {
        let jsonl = "{\"_key\": \"universe\", \"buildNumber\": 99999}\n";
        assert_eq!(find_sde_build_number(jsonl), None);
    }

    #[test]
    fn find_sde_build_number_skips_malformed_lines_without_aborting() {
        let jsonl = concat!(
            "not valid json\n",
            "\n", // blank line
            "{\"_key\": \"sde\", \"buildNumber\": 42}\n",
        );
        assert_eq!(find_sde_build_number(jsonl), Some("42".to_string()));
    }

    #[test]
    fn find_sde_build_number_accepts_string_build_numbers() {
        // in case CCP ever ships buildNumber as text in practice
        let jsonl = "{\"_key\": \"sde\", \"buildNumber\": \"12345\"}\n";
        assert_eq!(find_sde_build_number(jsonl), Some("12345".to_string()));
    }

    #[test]
    fn find_sde_build_number_handles_real_latest_jsonl() {
        // EXACT content of a real latest.jsonl (August 2026), with its
        // \r\n line ending as-is -- not hand-synthesized.
        let jsonl = "{\"_key\": \"sde\", \"buildNumber\": 3458726, \"releaseDate\": \"2026-08-06T11:07:36Z\"}\r\n";
        assert_eq!(find_sde_build_number(jsonl), Some("3458726".to_string()));
    }

    fn temp_data_dir(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("sde-index-test-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn update_as_needed_downloads_on_first_run() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/latest.jsonl"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_string("{\"_key\": \"sde\", \"buildNumber\": 123}\n"),
            )
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path(
                "/eve-online-static-data-123-jsonl.zip",
            ))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_bytes(b"zip content".to_vec()),
            )
            .mount(&server)
            .await;

        let client = http::build_client().unwrap();
        let data_dir = temp_data_dir("first_run");
        let base_url = format!("{}/", server.uri());

        let changed = update_as_needed(&client, &data_dir, &base_url, "jsonl")
            .await
            .unwrap();
        assert!(changed);

        let build = std::fs::read_to_string(data_dir.join("sde-jsonl.build")).unwrap();
        assert_eq!(build, "123");
        let zip_contents = std::fs::read(data_dir.join("sde-jsonl.zip")).unwrap();
        assert_eq!(zip_contents, b"zip content");
        assert!(!data_dir.join("sde-jsonl.zip.tmp").exists());
    }

    #[tokio::test]
    async fn update_as_needed_skips_when_build_matches() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/latest.jsonl"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_string("{\"_key\": \"sde\", \"buildNumber\": 123}\n"),
            )
            .mount(&server)
            .await;
        // The zip should never be requested -- explicitly expect 0 calls.
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path(
                "/eve-online-static-data-123-jsonl.zip",
            ))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;

        let client = http::build_client().unwrap();
        let data_dir = temp_data_dir("matches");
        std::fs::write(data_dir.join("sde-jsonl.build"), "123").unwrap();
        std::fs::write(data_dir.join("sde-jsonl.zip"), b"previous zip").unwrap();
        let base_url = format!("{}/", server.uri());

        let changed = update_as_needed(&client, &data_dir, &base_url, "jsonl")
            .await
            .unwrap();
        assert!(!changed);

        // The previous zip must not have been touched.
        let zip_contents = std::fs::read(data_dir.join("sde-jsonl.zip")).unwrap();
        assert_eq!(zip_contents, b"previous zip");
    }

    #[tokio::test]
    async fn update_as_needed_downloads_when_build_changed() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/latest.jsonl"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_string("{\"_key\": \"sde\", \"buildNumber\": 456}\n"),
            )
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path(
                "/eve-online-static-data-456-jsonl.zip",
            ))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_bytes(b"new zip".to_vec()))
            .mount(&server)
            .await;

        let client = http::build_client().unwrap();
        let data_dir = temp_data_dir("changed");
        std::fs::write(data_dir.join("sde-jsonl.build"), "123").unwrap();
        std::fs::write(data_dir.join("sde-jsonl.zip"), b"old zip").unwrap();
        let base_url = format!("{}/", server.uri());

        let changed = update_as_needed(&client, &data_dir, &base_url, "jsonl")
            .await
            .unwrap();
        assert!(changed);

        let build = std::fs::read_to_string(data_dir.join("sde-jsonl.build")).unwrap();
        assert_eq!(build, "456");
        let zip_contents = std::fs::read(data_dir.join("sde-jsonl.zip")).unwrap();
        assert_eq!(zip_contents, b"new zip");
    }

    #[tokio::test]
    async fn update_as_needed_returns_false_when_index_unreachable() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/latest.jsonl"))
            .respond_with(wiremock::ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let client = http::build_client().unwrap();
        let data_dir = temp_data_dir("unreachable");
        let base_url = format!("{}/", server.uri());

        let changed = update_as_needed(&client, &data_dir, &base_url, "jsonl")
            .await
            .unwrap();
        assert!(!changed);
    }
}
