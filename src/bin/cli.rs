//! CLI orchestrator for (re)building `sde.db`. Equivalent to the
//! top-level script body of `database_builder.py` -- see `main()`'s
//! docstring for the one deliberate behavioral difference. Requires the
//! `builder` feature.

use anyhow::Context;
use sde::builder::dotlan::DotlanConfig;
use sde::builder::parser::{ParserConfig, ProjectedAxis};
use sde::builder::{dotlan, extract, http, parser, schema, sde_index};
use std::path::PathBuf;

const SDE_URL: &str = "https://developers.eveonline.com/static-data/tranquility/";
const MAPS_URL: &str = "http://evemaps.dotlan.net/svg/";
const OUT_FILENAME: &str = "sde.db";
const SDE_VARIANT: &str = "jsonl";

/// (Re)builds `sde.db` from scratch whenever a new SDE build is
/// available. Equivalent to `database_builder.py`'s top-level script
/// body: `update_as_needed()` -> delete the old `sde.db` -> clean
/// `sde/` (preserving `maps/`) -> decompress the new zip ->
/// `SdeParser`/`ExternalParser`.
///
/// # Deliberate fix: rebuild is gated on `changed`, not unconditional
///
/// Python's script deletes `sde.db` and cleans `sde/` (except `maps/`)
/// on *every* run, regardless of `update_as_needed()`'s return value --
/// its own log message ("removing current sde database, because a
/// change was detected") only makes sense if that deletion were
/// conditional on `change`, but the code right above it never actually
/// checks it. The `if not OUT_FILENAME.exists()` guard that follows is
/// then always true (the file was just force-deleted), so the parser
/// re-runs on every single invocation, even when nothing changed --
/// wasted work (a full unzip + reparse of the whole SDE) for no
/// benefit, and a log message that's factually wrong on the runs where
/// nothing actually changed. Here, the whole rebuild (delete + clean +
/// unzip + parse + dotlan) only runs when `update_as_needed()` reports
/// a change, or `sde.db` doesn't exist yet.
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client = http::build_client().context("building the HTTP client")?;
    let data_dir = PathBuf::from("data");
    let sde_dir = PathBuf::from("sde");
    let out_path = PathBuf::from(OUT_FILENAME);

    let changed = sde_index::update_as_needed(&client, &data_dir, SDE_URL, SDE_VARIANT)
        .await
        .context("checking for a new SDE build")?;

    if !changed && out_path.exists() {
        println!("sde: {OUT_FILENAME} is already up to date, nothing to do");
        return Ok(());
    }

    if out_path.exists() {
        std::fs::remove_file(&out_path).context("removing the previous sde.db")?;
        println!("sde: removing the previous {OUT_FILENAME}, a new SDE build is available");
    }

    let zip_path = data_dir.join(format!("sde-{SDE_VARIANT}.zip"));
    extract::prepare_sde_directory(&zip_path, &sde_dir).context("decompressing the SDE zip")?;

    let mut connection = rusqlite::Connection::open(&out_path).context("creating sde.db")?;
    schema::create_schema(&connection).context("creating the schema")?;

    // Matches database_builder.py's SdeParser.configuration overrides.
    // `force_isometric_position_2d: true` because the old projX/Y/Z
    // system (what this script's settings originally targeted) always
    // computed the projection locally -- there was no "trust CCP's own
    // position2D" concept in Python at all, so setting this to `true`
    // is the faithful equivalent, not a new behavior.
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
    };
    let summary = parser::parse_data(&mut connection, &sde_dir, &parser_config)
        .context("parsing the SDE data")?;
    println!(
        "sde: parsed {} regions, {} constellations, {} solar systems, {} stargates, {} stars, {} planets, {} moons, {} connections",
        summary.regions,
        summary.constellations,
        summary.solar_systems,
        summary.stargates,
        summary.stars,
        summary.planets,
        summary.moons,
        summary.connections,
    );

    // Matches database_builder.py's ExternalParser.configuration overrides.
    let dotlan_config = DotlanConfig {
        with_icebelts: true,
        with_triglavian_status: true,
        with_jove_observatories: true,
        with_special_ore: true,
    };
    dotlan::process(&connection, &client, &sde_dir, MAPS_URL, &dotlan_config)
        .await
        .context("processing dotlan data")?;

    println!("sde: build complete -> {}", out_path.display());
    Ok(())
}
