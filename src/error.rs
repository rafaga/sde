//! Crate-wide error type.
//!
//! [`Error`] is the single error type returned by every fallible public
//! function in this crate: `SdeManager`'s read methods (always
//! compiled) and, with the `builder` feature enabled, everything under
//! [`crate::builder`] (HTTP, zip extraction, JSON parsing, and the SQL
//! writes that build `sde.db`).
//!
//! Before this module existed, the two halves of the crate disagreed on
//! what "an error" was: `SdeManager` returned bare `rusqlite::Error`,
//! while `builder` had its own `BuilderError` enum unifying IO/JSON/
//! HTTP/zip/SQLite. [`Error`] replaces both with one type, so a
//! consumer that uses both halves (or just wants a single `Result`
//! alias in their own code) doesn't have to juggle two error types.
//!
//! Its internal representation ([`ErrorKind`]) is deliberately private
//! instead of being a public enum of variants: which underlying crate a
//! given failure came from (rusqlite vs. reqwest vs. zip, etc.) is an
//! implementation detail that can change without that being a breaking
//! change for callers. Code that needs the concrete underlying error
//! (to match on a specific `rusqlite::Error` variant, for instance) can
//! still get it via [`std::error::Error::source`] and a `downcast_ref`.

use std::fmt;

/// The error type returned by every fallible function in this crate.
///
/// Implements [`std::error::Error`] (with [`std::error::Error::source`]
/// returning the underlying error, when there is one) and
/// [`std::fmt::Display`], so it composes normally with `?`, `anyhow`,
/// `Box<dyn std::error::Error>`, etc. See the module docs for why it
/// doesn't expose its variants directly.
#[derive(Debug)]
pub struct Error(ErrorKind);

/// Private: see the module docs on why this isn't `pub`.
#[derive(Debug)]
pub(crate) enum ErrorKind {
    /// Something went wrong talking to the SQLite database: a query
    /// failed, a required table/column is missing, a row didn't decode
    /// into the expected shape, etc.
    ///
    /// The only variant reachable without the `builder` feature, since
    /// `SdeManager`'s read path is the only thing compiled by default.
    Sqlite(rusqlite::Error),

    /// I/O error (reading/writing a file on disk).
    #[cfg(feature = "builder")]
    Io(std::io::Error),

    /// A file that was expected to contain JSON doesn't parse as such.
    #[cfg(feature = "builder")]
    Json(serde_json::Error),

    /// The HTTP client itself failed (DNS, TLS, a connection reset, a
    /// request timeout, etc.) -- as opposed to [`ErrorKind::HttpStatus`],
    /// where the server did respond.
    #[cfg(feature = "builder")]
    Http(reqwest::Error),

    /// The server responded, but with a non-2xx HTTP status.
    #[cfg(feature = "builder")]
    HttpStatus { url: String, status: u16 },

    /// Error reading/decompressing a zip file (see
    /// [`crate::builder::extract`]).
    #[cfg(feature = "builder")]
    Zip(zip::result::ZipError),

    /// An SDE record doesn't have the expected shape (a required field
    /// is missing, or is of a different type than expected). This isn't
    /// a JSON syntax error ([`ErrorKind::Json`] already covers that, for
    /// when the file doesn't even parse) nor an I/O error
    /// ([`ErrorKind::Io`]) -- the file was read and parsed fine, its
    /// content simply doesn't match what the parser needs.
    #[cfg(feature = "builder")]
    Data(String),
}

impl Error {
    /// Builds an [`ErrorKind::Data`] error: an SDE record that read and
    /// parsed fine but doesn't have the shape a parser needs (a
    /// required field is missing, or is of the wrong type).
    #[cfg(feature = "builder")]
    pub(crate) fn data(message: impl Into<String>) -> Self {
        Error(ErrorKind::Data(message.into()))
    }

    /// Builds an [`ErrorKind::HttpStatus`] error: the server responded,
    /// but with a non-2xx status.
    #[cfg(feature = "builder")]
    pub(crate) fn http_status(url: impl Into<String>, status: u16) -> Self {
        Error(ErrorKind::HttpStatus {
            url: url.into(),
            status,
        })
    }

    /// The kind of error this is. Crate-private: see the module docs on
    /// why `ErrorKind` itself isn't exported. Used internally where a
    /// caller genuinely needs to branch on the cause (e.g. tests
    /// asserting *which* failure a mock server response produced).
    #[allow(dead_code)]
    pub(crate) fn kind(&self) -> &ErrorKind {
        &self.0
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            ErrorKind::Sqlite(err) => write!(f, "SQLite error: {err}"),
            #[cfg(feature = "builder")]
            ErrorKind::Io(err) => write!(f, "I/O error: {err}"),
            #[cfg(feature = "builder")]
            ErrorKind::Json(err) => write!(f, "JSON error: {err}"),
            #[cfg(feature = "builder")]
            ErrorKind::Http(err) => write!(f, "HTTP error: {err}"),
            #[cfg(feature = "builder")]
            ErrorKind::HttpStatus { url, status } => {
                write!(f, "{url} responded with status {status}")
            }
            #[cfg(feature = "builder")]
            ErrorKind::Zip(err) => write!(f, "Zip error: {err}"),
            #[cfg(feature = "builder")]
            ErrorKind::Data(message) => write!(f, "malformed SDE record: {message}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.0 {
            ErrorKind::Sqlite(err) => Some(err),
            #[cfg(feature = "builder")]
            ErrorKind::Io(err) => Some(err),
            #[cfg(feature = "builder")]
            ErrorKind::Json(err) => Some(err),
            #[cfg(feature = "builder")]
            ErrorKind::Http(err) => Some(err),
            #[cfg(feature = "builder")]
            ErrorKind::Zip(err) => Some(err),
            #[cfg(feature = "builder")]
            ErrorKind::HttpStatus { .. } | ErrorKind::Data(_) => None,
        }
    }
}

impl From<rusqlite::Error> for Error {
    fn from(err: rusqlite::Error) -> Self {
        Error(ErrorKind::Sqlite(err))
    }
}

#[cfg(feature = "builder")]
impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Error(ErrorKind::Io(err))
    }
}

#[cfg(feature = "builder")]
impl From<serde_json::Error> for Error {
    fn from(err: serde_json::Error) -> Self {
        Error(ErrorKind::Json(err))
    }
}

#[cfg(feature = "builder")]
impl From<reqwest::Error> for Error {
    fn from(err: reqwest::Error) -> Self {
        Error(ErrorKind::Http(err))
    }
}

#[cfg(feature = "builder")]
impl From<zip::result::ZipError> for Error {
    fn from(err: zip::result::ZipError) -> Self {
        Error(ErrorKind::Zip(err))
    }
}
