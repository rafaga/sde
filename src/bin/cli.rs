//! CLI orchestrator for (re)building `sde.db`. Requires the
//! `builder` feature.

use anyhow::Context;
use clap::{Parser, Subcommand};
use sde::builder::parser::{ParserConfig, ProjectedAxis};
use sde::builder::{extract, http, parser, schema, sde_index};
use std::path::PathBuf;

const SDE_URL: &str = "https://developers.eveonline.com/static-data/tranquility/";
const MAPS_URL: &str = "http://evemaps.dotlan.net/svg/";
const SDE_VARIANT: &str = "jsonl";

#[derive(Parser)]
#[command(
    name = "sde-builder",
    version,
    about = "Builds/updates sde.db from EVE Online's Static Data Export"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Check for a new SDE build and rebuild the database if one is
    /// available (or if the database doesn't exist yet).
    Build {
        /// Rebuild even if the local database is already up to date.
        #[arg(long)]
        force: bool,
        /// Suppress the progress output the parser prints by
        /// default.
        #[arg(short, long)]
        quiet: bool,
        /// Path to write the database to.
        #[arg(short, long, default_value = "sde.db")]
        output: PathBuf,
        /// Also fetch and layer in community-maintained data on top of
        /// the canonical SDE (ice belts, Jove Observatories, Triglavian
        /// invasion status, special ore anomalies -- everything
        /// `builder::community` provides, including `mapAbstractSystems`,
        /// the one part of it that isn't gated by its own flag). Off by
        /// default: none of this comes from CCP's official export, so
        /// a plain `build` produces a database that's canonical SDE
        /// data only. Sets `ParserConfig.with_third_party` -- see
        /// [`parser::Parser::build_database`], which is what actually
        /// consults it; this binary itself makes no
        /// canonical-vs-third-party decision on its own.
        #[arg(long)]
        with_third_party: bool,
    },
}

/// (Re)builds the database from scratch whenever a new SDE build is
/// available: checks for an update, and if one exists (or the database
/// doesn't exist yet, or `--force` was passed), deletes the old
/// database, cleans `sde/` (preserving `maps/`), decompresses the new
/// zip, and parses it.
///
/// The whole rebuild (delete + clean + unzip + parse + community) only
/// runs when `update_as_needed()` reports a change, the database
/// doesn't exist yet, or `--force` was passed -- deliberately, to
/// avoid wasted work (a full unzip + reparse of the whole SDE) on runs
/// where nothing actually changed.
///
/// # This binary only turns flags into a `ParserConfig`
///
/// Whether to include `builder::community`'s community-maintained,
/// third-party data (see `--with-third-party` above) -- and everything
/// else about how the database gets built -- is decided by
/// [`parser::Parser::build_database`], not by this function. That's
/// deliberate: a library consumer calling `build_database` directly,
/// without going through this binary at all, gets the exact same
/// behavior for the exact same `ParserConfig`, since the decision
/// lives in one place instead of being duplicated (and potentially
/// drifting) between this binary and the library.
#[tokio::main]
#[tracing::instrument]
async fn main() -> anyhow::Result<()> {
    // Wire up `tracing` for the whole process before any span/event runs
    // anywhere in the dependency graph -- this binary is the top-level
    // orchestrator for the whole database-construction pipeline
    // (`sde_index` -> `extract` -> `schema` -> `parser::Parser::build_database`,
    // which drives every `parse_*` table and, with `--with-third-party`,
    // `community::process`), so this is the natural place to do it.
    // `TracyLayer::default()` starts the shared `tracy_client::Client`
    // itself (`Client::start()` is idempotent); open the Tracy desktop app
    // to connect, it auto-discovers the running process.
    #[cfg(feature = "profile-with-tracy")]
    {
        use tracing_subscriber::layer::SubscriberExt as _;
        tracing::subscriber::set_global_default(
            tracing_subscriber::registry().with(tracing_tracy::TracyLayer::default()),
        )
        .expect("setting the global tracing subscriber");
    }

    let cli = Cli::parse();
    let Command::Build {
        force,
        quiet,
        output,
        with_third_party,
    } = cli.command;

    let client = http::build_client().context("building the HTTP client")?;
    let data_dir = PathBuf::from("data");
    let sde_dir = PathBuf::from("sde");

    let changed = sde_index::update_as_needed(&client, &data_dir, SDE_URL, SDE_VARIANT)
        .await
        .context("checking for a new SDE build")?;

    if !force && !changed && output.exists() {
        println!(
            "sde: {} is already up to date, nothing to do",
            output.display()
        );
        return Ok(());
    }

    if output.exists() {
        std::fs::remove_file(&output).context("removing the previous database")?;
        println!(
            "sde: removing the previous {}, a new SDE build is available",
            output.display()
        );
    }

    let zip_path = data_dir.join(format!("sde-{SDE_VARIANT}.zip"));
    extract::prepare_sde_directory(&zip_path, &sde_dir).context("decompressing the SDE zip")?;

    let mut connection = rusqlite::Connection::open(&output).context("creating the database")?;
    schema::create_schema(&connection).context("creating the schema")?;

    // `force_isometric_position_2d: true`: this CLI always uses a
    // locally-computed isometric projection for `position2DX`/`Y`,
    // rather than trusting CCP's own precomputed `position2D` value.
    let parser_config = ParserConfig {
        language: "en".to_string(),
        force_isometric_position_2d: true,
        isometric_projected_axis: ProjectedAxis::Y,
        map_kspace: true,
        map_wspace: true,
        map_abyssal: true,
        map_void: true,
        with_gates: true,
        with_moons: true,
        verbose: !quiet,
        with_third_party,
    };
    let sde_parser = parser::Parser::new(&sde_dir, parser_config);
    let _summary = sde_parser
        .build_database(&mut connection, &client, MAPS_URL)
        .await
        .context("building the database")?;
    println!("sde: Parse complete");

    let third_party_note = if with_third_party {
        " (with community-maintained third-party data)"
    } else {
        " (canonical SDE only)"
    };
    println!(
        "sde: build complete{third_party_note} -> {}",
        output.display()
    );
    Ok(())
}
