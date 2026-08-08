# sde

A Rust library to read [EVE Online](https://www.eveonline.com/)'s Static
Data Export (SDE) from a SQLite database, plus an optional builder that
assembles that database from CCP's official SDE and additional
community-maintained sources.

This is the Rust counterpart of
[databaseCreator](https://github.com/rafaga/databaseCreator), the
original Python prototype this crate ports and extends.

## What's in the database

The database this crate reads (and can build) focuses on the shape of
EVE's universe and the items that exist in it: regions, constellations,
solar systems and their stargate connections, stars, planets and moons,
plus the basic item taxonomy (categories, groups, types), races,
factions and NPC corporations. A few extra layers of
community-maintained information ride on top of that map — things like
which systems have ice fields, which are Jove Observatories, and which
carry a Triglavian invasion status — kept separate from CCP's own data
rather than mixed into it.

It does not attempt to cover the SDE in full: broader datasets such as
blueprints and industry, market groups, dogma attributes/effects, and
similar are out of scope. The builder also only reads CCP's newer JSONL
export (not the YAML one), and only computes the isometric map
projection (not the dimetric one) when a system's 2D position isn't
already provided.

## Features

- **default** — read-only. Just `SdeManager` and the data types in
  `objects`, for consuming an already-built `sde.db`.
- **`builder`** — adds the pipeline that (re)builds `sde.db` from
  scratch. Installs the `sde-builder` CLI binary.
- **`gui`** — adds a GUI for the pipeline that (re)builds `sde.db` from
  scratch.(`sde-builder-gui`). Implies `builder`.

## Usage

```sh
cargo add sde
```

Read an existing `sde.db`:

```rust
use sde::SdeManager;
use std::path::Path;

let sde = SdeManager::new(Path::new("sde.db"), 1_000_000);
let points = sde.get_systempoints()?; // KdTree<f64, MapPoint, [f64; 3]>
let regions = sde.get_region_coordinates()?;
let connections = sde.get_connections()?; // RTree<MapSegment>, for spatial queries
```

## Building `sde.db`

With the `builder` feature enabled, the `sde` binary checks for a newer
SDE release, downloads and unpacks it if needed, and rebuilds the
database from it:

```sh
cargo run --bin sde --features builder
```

## Architecture

The crate has two parts. The core is a small, read-only API for
querying a database that already exists — this is what most consumers
of the crate will use. It indexes both map points and connections
spatially (a `KdTree` and an `RTree`, respectively), for queries like
"what's near this location" or "which connections fall within this
area" instead of a linear scan. Layered on top of that, behind the
`builder` feature, is a pipeline that produces that database in the
first place: fetching the source data, decompressing it, parsing it
into the schema, and folding in the extra community-provided layers
described above. The two parts are independent — nothing that only
reads the database needs any of the fetching/parsing machinery.

See [ERD.md](ERD.md) for the database's entity-relationship diagram.

## Related projects

- [databaseCreator](https://github.com/rafaga/databaseCreator) — the
  Python prototype this crate ports and extends.
- [egui-map](https://github.com/rafaga/egui-map) — the map-rendering
  widget `sde-gui` builds on.

## License

MIT
