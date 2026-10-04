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

/// The most recent build CCP lists in `latest.jsonl`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LatestBuild {
    pub build: String,
    /// `releaseDate` as published (RFC 3339, e.g. `2026-10-02T11:08:57Z`),
    /// if present.
    pub release_date: Option<String>,
}

/// Reads `{sde_url_base}latest.jsonl` and returns its most recent build,
/// or `None` if the file has no `sde` record. Unlike
/// [`update_as_needed`], a network failure is an `Err`.
#[tracing::instrument]
pub async fn fetch_latest(
    client: &Client,
    sde_url_base: &str,
) -> Result<Option<LatestBuild>, Error> {
    let contents = http::fetch_text(client, &format!("{sde_url_base}latest.jsonl")).await?;
    Ok(parse_latest(&contents))
}

/// The `_meta` record of CCP's changelog for one build
/// (`{sde_url_base}changes/<build>.jsonl`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangesMeta {
    pub build: u64,
    /// The build CCP's changelog compares against.
    pub last_build: u64,
    pub release_date: Option<String>,
}

/// Reads the `_meta` record of CCP's changelog for `build`.
#[tracing::instrument]
pub async fn fetch_changes_meta(
    client: &Client,
    sde_url_base: &str,
    build: u64,
) -> Result<ChangesMeta, Error> {
    let url = format!("{sde_url_base}changes/{build}.jsonl");
    let contents = http::fetch_text(client, &url).await?;
    parse_changes_meta(&contents)
        .ok_or_else(|| Error::data(format!("{url} has no usable `_meta` record")))
}

fn parse_changes_meta(jsonl: &str) -> Option<ChangesMeta> {
    let record = jsonl
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line.trim()).ok())
        .find(|record| record.get("_key").and_then(|v| v.as_str()) == Some("_meta"))?;
    Some(ChangesMeta {
        build: record.get("buildNumber")?.as_u64()?,
        last_build: record.get("lastBuildNumber")?.as_u64()?,
        release_date: record
            .get("releaseDate")
            .and_then(|v| v.as_str())
            .map(str::to_string),
    })
}

fn parse_latest(jsonl: &str) -> Option<LatestBuild> {
    let build = find_sde_build_number(jsonl)?;
    let release_date = jsonl
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line.trim()).ok())
        .find(|record| record.get("_key").and_then(|v| v.as_str()) == Some("sde"))
        .and_then(|record| record.get("releaseDate")?.as_str().map(str::to_string));
    Some(LatestBuild {
        build,
        release_date,
    })
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

    let index_url = format!("{sde_url_base}latest.jsonl");
    let index_contents = match http::fetch_text(client, &index_url).await {
        Ok(contents) => contents,
        Err(err) => {
            tracing::warn!("couldn't download {index_url} ({err})");
            return Ok(false);
        }
    };

    let Some(latest_build) = find_sde_build_number(&index_contents) else {
        tracing::warn!("couldn't determine the most recent build number in {index_url}");
        return Ok(false);
    };

    let current_build = std::fs::read_to_string(&build_file)
        .ok()
        .map(|s| s.trim().to_string());

    if current_build.as_deref() == Some(latest_build.as_str()) && zip_file.exists() {
        tracing::info!("{variant} data already up to date (build {latest_build})");
        return Ok(false);
    }

    tracing::info!(
        "new build available ({} -> {latest_build}), downloading {variant} data",
        current_build.as_deref().unwrap_or("none")
    );

    download_build(client, data_dir, sde_url_base, variant, &latest_build).await?;

    Ok(true)
}

/// Checks that `text` is an SDE build number: ASCII digits only, no sign,
/// spaces or leading zeros, and it fits a `u64` (the build numbers of CCP's
/// SDE and of sde-deltas). Returns it as the plain text the rest of this
/// module works with. Meant for validating user input before it ends up in
/// a URL or a file name.
pub fn parse_build_number(text: &str) -> Result<String, Error> {
    let text = text.trim();
    let valid = !text.is_empty()
        && text.bytes().all(|byte| byte.is_ascii_digit())
        && !text.starts_with('0')
        && text.parse::<u64>().is_ok();
    if valid {
        Ok(text.to_string())
    } else {
        Err(Error::data(format!(
            "`{text}` is not an SDE build number (digits only, e.g. 3569502)"
        )))
    }
}

/// Downloads the export of SDE build `build` (see [`parse_build_number`]) to
/// `<data_dir>/sde-{variant}.zip` and records the build in
/// `<data_dir>/sde-{variant}.build`, whether or not it's the latest one.
///
/// Like [`update_as_needed`], it downloads to a temporary file and only
/// replaces the previous zip once the download finished, so a failure (a
/// build CCP doesn't have, no network...) leaves what was there intact.
#[tracing::instrument]
pub async fn download_build(
    client: &Client,
    data_dir: &Path,
    sde_url_base: &str,
    variant: &str,
    build: &str,
) -> Result<(), Error> {
    parse_build_number(build)?;
    std::fs::create_dir_all(data_dir)?;
    let zip_file = data_dir.join(format!("sde-{variant}.zip"));
    let temp_zip_file = data_dir.join(format!("sde-{variant}.zip.tmp"));
    let zip_url = format!("{sde_url_base}eve-online-static-data-{build}-{variant}.zip");
    http::download(client, &zip_url, &temp_zip_file, |_| {}).await?;
    std::fs::rename(&temp_zip_file, &zip_file)?;
    std::fs::write(data_dir.join(format!("sde-{variant}.build")), build)?;
    Ok(())
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

    #[test]
    fn parse_latest_reads_the_build_and_its_release_date() {
        let jsonl = "{\"_key\": \"sde\", \"buildNumber\": 3458726, \"releaseDate\": \"2026-08-06T11:07:36Z\"}\r\n";
        assert_eq!(
            parse_latest(jsonl),
            Some(LatestBuild {
                build: "3458726".to_string(),
                release_date: Some("2026-08-06T11:07:36Z".to_string()),
            })
        );
        assert_eq!(
            parse_latest("{\"_key\": \"sde\", \"buildNumber\": 1}")
                .unwrap()
                .release_date,
            None
        );
        assert_eq!(parse_latest(""), None);
    }

    #[test]
    fn parse_changes_meta_reads_the_meta_record() {
        // First two lines of the real changes/3569502.jsonl (October 2026).
        let jsonl = concat!(
            "{\"_key\":\"_meta\",\"buildNumber\":3569502,\"lastBuildNumber\":3561556,",
            "\"releaseDate\":\"2026-10-02T11:08:57Z\"}\n",
            "{\"_key\":\"missions\",\"changedLocalization\":[4843]}\n"
        );
        assert_eq!(
            parse_changes_meta(jsonl),
            Some(ChangesMeta {
                build: 3569502,
                last_build: 3561556,
                release_date: Some("2026-10-02T11:08:57Z".to_string()),
            })
        );
        assert_eq!(parse_changes_meta("{\"_key\":\"types\"}"), None);
    }

    fn temp_data_dir(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("sde-index-test-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn parse_build_number_accepts_only_plain_build_numbers() {
        assert_eq!(parse_build_number("3569502").unwrap(), "3569502");
        assert_eq!(parse_build_number("  42\n").unwrap(), "42");
        for text in [
            "",
            "0",
            "0123",
            "-5",
            "+5",
            "12 34",
            "1e5",
            "../1",
            "1/2",
            "latest",
            "18446744073709551616",
        ] {
            assert!(parse_build_number(text).is_err(), "{text:?} was accepted");
        }
    }

    #[tokio::test]
    async fn download_build_gets_the_requested_build_and_keeps_the_zip_on_failure() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::path(
            "/eve-online-static-data-77-jsonl.zip",
        ))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_bytes(b"build 77".to_vec()))
        .mount(&server)
        .await;
        let client = http::build_client().unwrap();
        let data_dir = temp_data_dir("download_build");
        let base_url = format!("{}/", server.uri());

        download_build(&client, &data_dir, &base_url, "jsonl", "77")
            .await
            .unwrap();
        assert_eq!(
            std::fs::read(data_dir.join("sde-jsonl.zip")).unwrap(),
            b"build 77"
        );
        assert_eq!(
            std::fs::read_to_string(data_dir.join("sde-jsonl.build")).unwrap(),
            "77"
        );

        // A build CCP doesn't have (404), or a name that isn't a build.
        assert!(
            download_build(&client, &data_dir, &base_url, "jsonl", "78")
                .await
                .is_err()
        );
        assert!(
            download_build(&client, &data_dir, &base_url, "jsonl", "../x")
                .await
                .is_err()
        );
        assert_eq!(
            std::fs::read(data_dir.join("sde-jsonl.zip")).unwrap(),
            b"build 77"
        );
        assert_eq!(
            std::fs::read_to_string(data_dir.join("sde-jsonl.build")).unwrap(),
            "77"
        );
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
