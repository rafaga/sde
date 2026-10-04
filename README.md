# sde

A Rust library to read [EVE Online](https://www.eveonline.com/)'s Static
Data Export (SDE) from a SQLite database, plus an optional builder that
assembles that database from CCP's official SDE and additional
community-maintained sources.

This is the Rust port of [databaseCreator](https://github.com/rafaga/databaseCreator),
the original Python prototype where this crate gets inspired. Much
of the original algorithm was replaced with better abstractions
or with better design practices

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

## Usage

```sh
cargo add sde
```

Read an existing `sde.db`:

```rust
use sde::SdeManager;
use std::path::Path;

let sde = SdeManager::new(Path::new("sde.db"), 1_000_000);
let points = sde.get_systems()?; // KdTree<f64, SdePoint, [f64; 3]>
let regions = sde.get_region_coordinates()?;
let connections = sde.get_connections()?; // RTree<SdeSegment>, for spatial queries
```

## Building `sde.db`

With the `builder` feature enabled, the `sde-builder` binary has two
commands:

```sh
# Build the database from scratch from CCP's export (the latest SDE
# build, or a specific one), even if it's already up to date:
cargo run --bin sde-builder --features builder -- build [--sde-build 3569502]

# Bring the database up to the latest SDE build the cheapest way:
cargo run --bin sde-builder --features builder -- update
```

Both take `-o`/`--output <path>` (the database, `sde.db` by default),
`--data-dir` and `--sde-dir`, `--with-third-party`, `--keep-source` and
`-q`/`--quiet`; `update` also takes `--ignore-delta-lag`. See `--help` of
each for the full list. The binary only validates its arguments and calls
`builder::pipeline::build` / `builder::pipeline::update`, which a library
consumer can call too.

### What stays on disk

Once a build finishes, nothing downloaded from CCP or decompressed is
left. A build downloads CCP's zip (~99 MB) into `data/`, decompresses it
into `sde/` (~560 MB), builds the database, and then:

- the zip, the file recording its build and the decompressed tables are
  removed (`pipeline::clean_downloads`);
- what the parser read is kept as the *mirror* in a single zip,
  `data/sde-mirror.zip` (~20 MB), so delta updates can work;
- `sde/` keeps only `maps/`, dotlan's maps, which come from another source
  and aren't downloaded again unless they changed.

So what remains is `sde.db`, `data/sde-mirror.zip` and `sde/maps/`.
`--keep-source` skips all of this and keeps CCP's export as it is.

### Delta updates

The mirror holds only the tables and fields the parser actually read,
recorded while it built the database. `update` brings it up to date with
the build-to-build deltas published by
[sde-deltas](https://github.com/rafaga/sde-deltas), usually a few KB per
build, and then:

- if the database is already at that build, nothing is unpacked and
  nothing is done;
- if no change touches a field the parser reads, only the build recorded
  in the database's fingerprint moves -- no rebuild;
- otherwise the mirror is unpacked into `sde/`, the database is rebuilt
  from it without downloading CCP's export, and `sde/` is emptied again;
- if the deltas can't be used (no mirror yet, a build they don't cover,
  a schema change in a field the parser reads, an inconsistency -- including
  a table whose record count differs from the one sde-deltas publishes --,
  or sde-deltas lagging more than two days behind CCP), it falls back to a
  full build from CCP's export, which makes a new mirror.

A mirror an earlier version (0.6.x) left unpacked in `sde/` is packed into
`data/sde-mirror.zip` the first time `update` runs.

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

- [databaseCreator](https://github.com/rafaga/databaseCreator) — the original
  Python prototype where this crate gets inspired.

## License

MIT
