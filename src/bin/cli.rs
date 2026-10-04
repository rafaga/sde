//! CLI for building and updating `sde.db`. Requires the `builder` feature.
//!
//! This binary only turns its arguments into a [`Pipeline`] and calls
//! [`pipeline::build`] / [`pipeline::update`]; what those do, and every
//! decision behind it, lives in `sde::builder::pipeline`, so a library
//! consumer calling them gets the exact same behavior.

use anyhow::Context;
use clap::{Args, Parser, Subcommand};
use sde::builder::BuildUrls;
use sde::builder::http;
use sde::builder::parser::{ParserConfig, Position2DMode, ProjectedAxis};
use sde::builder::pipeline::{self, Pipeline, Workspace};
use sde::builder::sde_index;
use sde::builder::update::UpdateOptions;
use std::path::PathBuf;

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
    /// Build the database from scratch from CCP's export, whether or not
    /// it's already up to date: the latest SDE build, or the one given with
    /// `--sde-build`.
    Build {
        /// SDE build number to build from (e.g. 3569502) instead of the
        /// latest one.
        #[arg(long, value_name = "BUILD", value_parser = build_number)]
        sde_build: Option<String>,
        #[command(flatten)]
        common: Common,
    },
    /// Bring the database up to the latest SDE build the cheapest way: with
    /// sde-deltas' build-to-build deltas (nothing to do, only the recorded
    /// build moves, or a rebuild from the local mirror of the SDE left by
    /// the previous build, without downloading CCP's export), and from
    /// CCP's full export only when the deltas can't be used.
    Update {
        /// Use sde-deltas however far behind CCP it is, instead of falling
        /// back to a full build when it lags for more than two days.
        #[arg(long)]
        ignore_delta_lag: bool,
        #[command(flatten)]
        common: Common,
    },
}

/// What both commands take.
#[derive(Args)]
struct Common {
    /// Path of the database.
    #[arg(short, long, default_value = "sde.db", value_parser = not_empty_path)]
    output: PathBuf,
    /// Where CCP's zip is downloaded to while building, and where the mirror
    /// of the SDE that delta updates need is kept, as one zip
    /// (`sde-mirror.zip`, ~20 MB).
    #[arg(long, default_value = "data", value_parser = not_empty_path)]
    data_dir: PathBuf,
    /// Where the SDE files are decompressed to while building. Only dotlan's
    /// `maps/` is kept in it afterwards: everything else in it is deleted by
    /// a build.
    #[arg(long, default_value = "sde", value_parser = not_empty_path)]
    sde_dir: PathBuf,
    /// After a full build, keep CCP's export (the SDE directory and the
    /// zip) as is: no mirror is made and nothing is removed. The next
    /// update is then a full build again.
    #[arg(long)]
    keep_source: bool,
    /// Also fetch and layer in community-maintained data on top of the
    /// canonical SDE (ice belts, Jove Observatories, Triglavian invasion
    /// status, special ore anomalies, `mapAbstractSystems`). Off by default:
    /// none of this comes from CCP's official export.
    #[arg(long)]
    with_third_party: bool,
    /// Suppress the progress output the parser prints by default.
    #[arg(short, long)]
    quiet: bool,
}

fn build_number(text: &str) -> Result<String, String> {
    sde_index::parse_build_number(text).map_err(|error| error.to_string())
}

fn not_empty_path(text: &str) -> Result<PathBuf, String> {
    if text.trim().is_empty() {
        Err("the path is empty".to_string())
    } else {
        Ok(PathBuf::from(text))
    }
}

impl Common {
    /// The pipeline these arguments describe.
    fn pipeline(&self, updates: UpdateOptions) -> anyhow::Result<Pipeline> {
        let workspace = Workspace::new(
            self.output.clone(),
            self.data_dir.clone(),
            self.sde_dir.clone(),
        );
        workspace.validate().context("invalid paths")?;
        Ok(Pipeline {
            workspace,
            urls: BuildUrls::default(),
            // Local projection instead of CCP's precomputed `position2D`:
            // that value is a hand-adjusted schematic of the in-game map,
            // not a projection of the 3D coordinates, and covers k-space
            // only. `Orthogonal(Y)` is the north-up top-down: EVE's
            // galactic plane is the X-Z plane (x = east, z = north) with y
            // as the vertical axis, so dropping y gives east = screen
            // right, north = screen up -- the community-canonical
            // orientation -- and, being a true projection, it covers every
            // system in scope (w-space included).
            parser: ParserConfig {
                language: "en".to_string(),
                position_2d: Position2DMode::Orthogonal(ProjectedAxis::Y),
                map_kspace: true,
                map_wspace: true,
                map_abyssal: true,
                map_void: true,
                with_gates: true,
                with_moons: true,
                verbose: !self.quiet,
                with_third_party: self.with_third_party,
            },
            keep_source: self.keep_source,
            updates,
        })
    }
}

#[tokio::main]
#[tracing::instrument]
async fn main() -> anyhow::Result<()> {
    // Wire up `tracing` for the whole process before any span/event runs
    // anywhere in the dependency graph: every `tracing::info!`/`warn!`/etc.
    // the library emits is silently dropped without a subscriber registered
    // somewhere, and a library consumer registers its own (or none at all).
    // This binary always registers a console-printing one (`fmt::layer()`).
    //
    // `TracyLayer::default()` starts the shared `tracy_client::Client`
    // itself (`Client::start()` is idempotent); open the Tracy desktop app
    // to connect, it auto-discovers the running process.
    {
        use tracing_subscriber::layer::SubscriberExt as _;
        let registry = tracing_subscriber::registry().with(tracing_subscriber::fmt::layer());
        #[cfg(feature = "profile")]
        let registry = registry.with(tracing_tracy::TracyLayer::default());
        tracing::subscriber::set_global_default(registry)
            .expect("setting the global tracing subscriber");
    }

    let client = http::build_client().context("building the HTTP client")?;
    let say = |event: pipeline::Event| println!("sde: {event}");
    let outcome = match Cli::parse().command {
        Command::Build { sde_build, common } => {
            let pipeline = common.pipeline(UpdateOptions::default())?;
            pipeline::build(&client, &pipeline, sde_build.as_deref(), say)
                .await
                .context("building the database")?
        }
        Command::Update {
            ignore_delta_lag,
            common,
        } => {
            let mut updates = UpdateOptions::default();
            if ignore_delta_lag {
                updates.max_delta_lag_seconds = None;
            }
            let pipeline = common.pipeline(updates)?;
            pipeline::update(&client, &pipeline, say)
                .await
                .context("updating the database")?
        }
    };
    println!("sde: {outcome}");
    Ok(())
}
