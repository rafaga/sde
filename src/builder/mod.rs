//! Logic to (re)generate `sde.db` from CCP's official SDE and dotlan's
//! maps.
//!
//! This whole module lives behind the `builder` feature (disabled by
//! default) so that a consumer that only *reads* `sde.db` -- like the
//! project's main egui app -- doesn't drag in reqwest/tokio/zip/etc.
//! The `sde` (CLI) and `sde-gui` binaries enable this feature and call
//! into this module's functions.

pub mod dotlan;
pub mod extract;
pub mod http;
pub mod manifest;
pub mod parser;
pub mod schema;
pub mod sde_index;

// `schema` (STRICT DDL) is already ported -- see builder::schema::create_schema().
// `parser` (data writing) is fully ported -- 14 functions, full parity
// with Python's parse_data(). See builder::parser's docstring for the
// phase-by-phase detail.
// `dotlan` (community data external to the SDE) is fully ported --
// dynamic DDL, static list population, SVG parsing (validated against a
// real dotlan map) and the download orchestrator with retries. See
// builder::dotlan's docstring for the detail.
// `sde_index` (build number check + conditional SDE download) is
// already ported -- see builder::sde_index's docstring.
// `extract` (SDE zip decompression, preserving maps/) is already
// ported -- see builder::extract's docstring.
//
// The only thing missing is the top-level orchestrator that ties all of
// this together (today a stub in src/bin/cli.rs):
// sde_index::update_as_needed() -> extract::prepare_sde_directory() ->
// parser::parse_data() -> dotlan::process(), in that order -- the same
// one database_builder.py follows.

/// Build process errors. Deliberately without `thiserror`: same
/// "no abstraction" pattern `SdeManager` already uses (propagates the
/// underlying crates' errors as-is with `?`), except here we do need to
/// unify several different error types (HTTP, IO, zip, JSON, sqlite).
#[derive(Debug)]
pub enum BuilderError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Http(reqwest::Error),
    /// The server responded, but with a non-2xx HTTP status.
    HttpStatus { url: String, status: u16 },
    /// Error running a SQL query (schema creation, parser inserts,
    /// etc.).
    Sqlite(rusqlite::Error),
    /// Error reading/decompressing a zip file (`builder::extract`).
    Zip(zip::result::ZipError),
    /// An SDE record doesn't have the expected shape (a required field
    /// is missing, or is of a different type than expected). This isn't
    /// a JSON syntax error (`Json` already covers that, for when the
    /// file doesn't even parse) nor an I/O error (`Io`) -- the file was
    /// read and parsed fine, its content simply doesn't match what the
    /// parser needs.
    Data(String),
}

impl std::fmt::Display for BuilderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuilderError::Io(err) => write!(f, "I/O error: {err}"),
            BuilderError::Json(err) => write!(f, "JSON error: {err}"),
            BuilderError::Http(err) => write!(f, "HTTP error: {err}"),
            BuilderError::HttpStatus { url, status } => {
                write!(f, "{url} responded with status {status}")
            },
            BuilderError::Sqlite(err) => write!(f, "SQLite error: {err}"),
            BuilderError::Zip(err) => write!(f, "Zip error: {err}"),
            BuilderError::Data(message) => write!(f, "malformed SDE record: {message}"),
        }
    }
}

impl std::error::Error for BuilderError {}

impl From<std::io::Error> for BuilderError {
    fn from(err: std::io::Error) -> Self {
        BuilderError::Io(err)
    }
}

impl From<rusqlite::Error> for BuilderError {
    fn from(err: rusqlite::Error) -> Self {
        BuilderError::Sqlite(err)
    }
}

impl From<zip::result::ZipError> for BuilderError {
    fn from(err: zip::result::ZipError) -> Self {
        BuilderError::Zip(err)
    }
}

impl From<serde_json::Error> for BuilderError {
    fn from(err: serde_json::Error) -> Self {
        BuilderError::Json(err)
    }
}

impl From<reqwest::Error> for BuilderError {
    fn from(err: reqwest::Error) -> Self {
        BuilderError::Http(err)
    }
}
