//! Logic to (re)generate `sde.db` from CCP's official SDE and dotlan's
//! maps.
//!
//! This whole module lives behind the `builder` feature (disabled by
//! default) so that a consumer that only *reads* `sde.db` doesn't drag
//! in reqwest/tokio/zip/etc. The `sde-builder` binary enables this
//! feature and calls into this module's functions.

pub mod community;
pub mod extract;
pub mod http;
pub mod manifest;
pub mod parser;
pub mod schema;
pub mod sde_index;

// `schema` (STRICT DDL): see builder::schema::create_schema().
// `parser` (data writing): see builder::parser's docstring for the
// full list of tables it covers.
// `community` (community data external to the SDE): dynamic DDL, static
// list population, SVG parsing, and the download orchestrator with
// retries. See builder::community's docstring for the detail.
// `sde_index` (build number check + conditional SDE download): see
// builder::sde_index's docstring.
// `extract` (SDE zip decompression, preserving maps/): see
// builder::extract's docstring.
//
// The top-level orchestrator that ties all of this together lives in
// src/bin/cli.rs's `main()`: sde_index::update_as_needed() ->
// extract::prepare_sde_directory() -> parser::Parser::build_database()
// (which itself runs parse_data(), then community::process() only if
// `--with-third-party` was passed). [`BuildUrls`] holds the default
// endpoints that orchestration fetches from.

/// Default network endpoints for a full build: CCP's own SDE export
/// (`sde_url`/`sde_variant`, consumed by
/// [`sde_index::update_as_needed`]) and the third-party map data used
/// when `ParserConfig.with_third_party` is set (`maps_url`, consumed
/// by [`parser::Parser::build_database`]).
///
/// A plain struct with a [`Default`] impl rather than free-standing
/// constants, so any caller assembling a build pipeline around this
/// crate -- this crate's own `sde-builder` binary included, not just
/// external library consumers -- has something to construct and
/// override piecemeal (a private SDE mirror, a different map source)
/// instead of three separate constants with no shared identity.
///
/// ```
/// # use sde::builder::BuildUrls;
/// let urls = BuildUrls::default();
/// assert_eq!(urls.sde_variant, "jsonl");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildUrls {
    pub sde_variant: String,
    pub sde_url: String,
    pub maps_url: String,
}

impl Default for BuildUrls {
    fn default() -> Self {
        Self {
            sde_variant: "jsonl".to_string(),
            sde_url: "https://developers.eveonline.com/static-data/tranquility/".to_string(),
            maps_url: "http://evemaps.dotlan.net/svg/".to_string(),
        }
    }
}

/// Build process errors.
///
/// This used to be its own enum (`Io`/`Json`/`Http`/`HttpStatus`/
/// `Sqlite`/`Zip`/`Data`), deliberately without `thiserror`, unifying
/// the several error types the build pipeline touches (HTTP, IO, zip,
/// JSON, sqlite). It's now a type alias for [`crate::Error`], the same
/// crate-wide error type [`crate::SdeManager`]'s read methods return --
/// see `src/error.rs` for why the two were merged and why the
/// underlying representation is private now.
///
/// Kept (rather than removed outright) so downstream code that still
/// does `use sde::builder::BuilderError` keeps compiling; new code
/// should use [`crate::Error`] directly.
#[deprecated(
    since = "0.3.0",
    note = "use `sde::Error` instead -- `BuilderError` and `SdeManager`'s \
            read-path errors are the same type now"
)]
pub type BuilderError = crate::Error;
