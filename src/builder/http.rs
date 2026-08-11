//! Thin HTTP client on top of `reqwest`, used both to check whether a
//! dotlan map changed (`fingerprint`) and to download the SDE and SVG
//! files (`download`).
//!
//! Async, and able to run many checks in parallel (`fingerprint_many`)
//! instead of one at a time.

use crate::builder::BuilderError;
use crate::builder::manifest::MapFingerprint;
use futures::StreamExt;
use reqwest::Client;
use reqwest::header::{HeaderMap, HeaderName};
use std::collections::HashMap;
use std::path::Path;
use tokio::io::AsyncWriteExt;

/// Progress of an ongoing download: bytes downloaded so far and, if the
/// server reported it, the expected total (to draw a determinate
/// progress bar).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DownloadProgress {
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
}

/// Builds the `Client` that should be reused for every call (reqwest
/// pools connections internally; creating a new one per request throws
/// that advantage away).
pub fn build_client() -> reqwest::Result<Client> {
    Client::builder()
        .user_agent(concat!(
            env!("CARGO_PKG_NAME"),
            "/",
            env!("CARGO_PKG_VERSION")
        ))
        .build()
}

fn header_string(headers: &HeaderMap, name: HeaderName) -> Option<String> {
    headers.get(name)?.to_str().ok().map(str::to_string)
}

/// Does a `HEAD` request to `url` and builds its fingerprint
/// (ETag/Last-Modified/Content-Length). Returns `None` if it couldn't be
/// verified -- no network, timeout, or a non-2xx status -- so a
/// one-off network hiccup doesn't block the build.
pub async fn fingerprint(client: &Client, url: &str) -> Option<MapFingerprint> {
    let response = match client.head(url).send().await {
        Ok(resp) => resp,
        Err(err) => {
            eprintln!("http: {url} can't be verified ({err})");
            return None;
        }
    };
    if !response.status().is_success() {
        eprintln!(
            "http: HEAD {url} responded with status {}",
            response.status()
        );
        return None;
    }
    let headers = response.headers();
    Some(MapFingerprint {
        etag: header_string(headers, reqwest::header::ETAG),
        last_modified: header_string(headers, reqwest::header::LAST_MODIFIED),
        content_length: header_string(headers, reqwest::header::CONTENT_LENGTH),
    })
}

/// Runs `fingerprint()` for many URLs in parallel, with a concurrency
/// cap (so we don't hammer dotlan with hundreds of simultaneous
/// requests). `items` are `(key, url)` pairs; the key is typically the
/// region name, and it's returned as-is so it can be cross-referenced
/// against the manifest.
pub async fn fingerprint_many(
    client: &Client,
    items: impl IntoIterator<Item = (String, String)>,
    concurrency: usize,
) -> HashMap<String, Option<MapFingerprint>> {
    futures::stream::iter(items)
        .map(|(key, url)| {
            let client = client.clone();
            async move {
                let fp = fingerprint(&client, &url).await;
                (key, fp)
            }
        })
        .buffer_unordered(concurrency.max(1))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect()
}

/// Downloads `url` and returns its body as text -- meant for small
/// responses that are better kept in memory instead of written to disk
/// (e.g. `latest.jsonl`, the SDE build-number index consumed by
/// [`crate::builder::sde_index`]). Unlike [`download`], it doesn't write
/// anything to disk nor report progress.
pub async fn fetch_text(client: &Client, url: &str) -> Result<String, BuilderError> {
    let response = client.get(url).send().await?;
    if !response.status().is_success() {
        return Err(BuilderError::HttpStatus {
            url: url.to_string(),
            status: response.status().as_u16(),
        });
    }
    Ok(response.text().await?)
}

/// Downloads `url` to `destination` (overwriting it if it already
/// exists), reporting progress per chunk via `on_progress`. Returns the
/// total bytes downloaded.
pub async fn download(
    client: &Client,
    url: &str,
    destination: &Path,
    mut on_progress: impl FnMut(DownloadProgress),
) -> Result<u64, BuilderError> {
    let response = client.get(url).send().await?;
    if !response.status().is_success() {
        return Err(BuilderError::HttpStatus {
            url: url.to_string(),
            status: response.status().as_u16(),
        });
    }
    let total_bytes = response.content_length();

    if let Some(parent) = destination.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut file = tokio::fs::File::create(destination).await?;

    let mut downloaded_bytes = 0u64;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        file.write_all(&chunk).await?;
        downloaded_bytes += chunk.len() as u64;
        on_progress(DownloadProgress {
            downloaded_bytes,
            total_bytes,
        });
    }
    file.flush().await?;
    Ok(downloaded_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn fingerprint_reads_headers_on_success() {
        let server = MockServer::start().await;
        Mock::given(method("HEAD"))
            .and(path("/The_Forge.svg"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("ETag", "\"abc123\"")
                    .insert_header("Last-Modified", "Wed, 15 Jan 2026 10:32:00 GMT")
                    .insert_header("Content-Length", "184320"),
            )
            .mount(&server)
            .await;

        let client = build_client().unwrap();
        let url = format!("{}/The_Forge.svg", server.uri());
        let fp = fingerprint(&client, &url)
            .await
            .expect("should return Some");

        assert_eq!(fp.etag.as_deref(), Some("\"abc123\""));
        assert_eq!(fp.content_length.as_deref(), Some("184320"));
    }

    #[tokio::test]
    async fn fingerprint_is_none_on_404() {
        let server = MockServer::start().await;
        Mock::given(method("HEAD"))
            .and(path("/Does_Not_Exist.svg"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let client = build_client().unwrap();
        let url = format!("{}/Does_Not_Exist.svg", server.uri());
        assert!(fingerprint(&client, &url).await.is_none());
    }

    #[tokio::test]
    async fn fetch_text_returns_body_on_success() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/latest.jsonl"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{\"_key\":\"sde\"}\n"))
            .mount(&server)
            .await;

        let client = build_client().unwrap();
        let url = format!("{}/latest.jsonl", server.uri());
        let text = fetch_text(&client, &url).await.unwrap();
        assert_eq!(text, "{\"_key\":\"sde\"}\n");
    }

    #[tokio::test]
    async fn fetch_text_errors_on_non_success_status() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/latest.jsonl"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let client = build_client().unwrap();
        let url = format!("{}/latest.jsonl", server.uri());
        let result = fetch_text(&client, &url).await;
        assert!(matches!(
            result,
            Err(BuilderError::HttpStatus { status: 404, .. })
        ));
    }

    #[tokio::test]
    async fn fingerprint_many_pairs_results_with_their_key() {
        let server = MockServer::start().await;
        Mock::given(method("HEAD"))
            .and(path("/Domain.svg"))
            .respond_with(ResponseTemplate::new(200).insert_header("ETag", "\"domain\""))
            .mount(&server)
            .await;
        Mock::given(method("HEAD"))
            .and(path("/Impass.svg"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let client = build_client().unwrap();
        let items = vec![
            ("Domain".to_string(), format!("{}/Domain.svg", server.uri())),
            ("Impass".to_string(), format!("{}/Impass.svg", server.uri())),
        ];
        let results = fingerprint_many(&client, items, 4).await;

        assert!(results.get("Domain").unwrap().is_some());
        assert!(results.get("Impass").unwrap().is_none());
    }

    #[tokio::test]
    async fn download_writes_body_to_destination() {
        let server = MockServer::start().await;
        let body = b"test svg content".to_vec();
        Mock::given(method("GET"))
            .and(path("/Domain.svg"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body.clone()))
            .mount(&server)
            .await;

        let client = build_client().unwrap();
        let url = format!("{}/Domain.svg", server.uri());
        let dir = std::env::temp_dir().join(format!("sde-http-test-{}", std::process::id()));
        let destination = dir.join("Domain.svg");

        let mut last_progress = DownloadProgress::default();
        let total = download(&client, &url, &destination, |p| last_progress = p)
            .await
            .expect("download should succeed");

        assert_eq!(total, body.len() as u64);
        assert_eq!(last_progress.downloaded_bytes, body.len() as u64);
        let written = std::fs::read(&destination).unwrap();
        assert_eq!(written, body);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn download_errors_on_non_success_status() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/Does_Not_Exist.svg"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let client = build_client().unwrap();
        let url = format!("{}/Does_Not_Exist.svg", server.uri());
        let dir = std::env::temp_dir().join(format!("sde-http-test-err-{}", std::process::id()));
        let destination = dir.join("Does_Not_Exist.svg");

        let result = download(&client, &url, &destination, |_| {}).await;
        assert!(matches!(
            result,
            Err(BuilderError::HttpStatus { status: 500, .. })
        ));
    }
}
