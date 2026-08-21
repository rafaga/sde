//! Logic to (re)generate `sde.db` from CCP's official SDE and dotlan's
//! maps.
//!
//! This whole module lives behind the `builder` feature (disabled by
//! default) so that a consumer that only *reads* `sde.db` -- like the
//! project's main egui app -- doesn't drag in reqwest/tokio/zip/etc.
//! The `sde` (CLI) and `sde-gui` binaries enable this feature and call
//! into this module's functions.

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
// `--with-third-party` was passed).

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
