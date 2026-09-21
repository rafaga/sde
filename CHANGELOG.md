# Changelog

All notable changes to the `sde` crate (rafaga/sde) are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the
project uses [Semantic Versioning](https://semver.org/). Published versions (tags
`0.0.3` … `0.3.3`) were reconstructed from `git log` history and from `Cargo.toml` at
each tag; the **[Unreleased]** section covers commits that exist on the current HEAD of
the `test` branch (origin/test) on top of the latest tag and that don't yet have a
version bump in `Cargo.toml`.

## [0.3.3] — 2026-09-21

### Added

- New `Position2DMode` enum (`Ccp` / `Isometric(axis)` / `Orthogonal(axis)`) that
  replaces the `force_isometric_position_2d: bool` + `isometric_projected_axis` field
  pair in `ParserConfig` and in `SdeFingerprint`.
- New `orthogonal_projection_2d` function: a true orthographic projection (drops one
  axis instead of projecting it isometrically), used to get the "north-up, top-down"
  orientation the community treats as the standard for the EVE map.
- `Position2DMode::fingerprint_columns` / `from_fingerprint_columns` to encode and
  decode the new mode into `sdeFingerprint`'s existing `forceIsometricPosition2d` /
  `isometricProjectedAxis` columns (no schema change required).
- New tests for `orthogonal_projection_2d`, for the `Orthogonal` mode in
  `parse_solar_systems`, and for the Y-only coordinate inversion.

### Changed

- **API change**: `ParserConfig.force_isometric_position_2d` +
  `ParserConfig.isometric_projected_axis` are replaced by a single
  `ParserConfig.position_2d: Position2DMode` field (defaults to
  `Position2DMode::Ccp`, the same behavior the old `false` had).
- `SdeFingerprint` now exposes `position_2d: Position2DMode` instead of its previous
  two fields; this changes `to_hash_input`'s format, so fingerprints written by older
  versions of the crate no longer verify (considered acceptable: any config change
  already implies a rebuild).
- The `sde-builder` binary (`src/bin/cli.rs`) no longer always forces an isometric
  projection and now defaults to `Position2DMode::Orthogonal(ProjectedAxis::Y)`.
- Coordinate inversion (`SdeManager::invert_coordinates`) for 2D map coordinates
  (`get_systems`, `get_connections`, `get_region_coordinates`,
  `SolarSystem::projected_coords`) now flips only the Y component, not X — real 3D
  coordinates (`get_system_coords`, `SolarSystem::real_coords`) still flip all three
  axes, unchanged.

### Fixed

- Coordinate inversion also flipped X on 2D projections, which mirrored the universe
  east-west on the map; it now flips only Y (the axis that points down in screen
  space), preserving the correct east/west orientation.
- `setup_special_anomalies` (`src/builder/community.rs`) joined `typeStar` to
  `mapStars` using the nonexistent `ts.starTypeId` column (that column lives on
  `mapStars`, not on `typeStar`, whose primary key is `typeId`) — a reference to an
  already-removed foreign key. The join now uses `ts.typeId = m.starTypeId`, fixing
  both the function and its unit test. This bug affected real builds whenever
  `CommunityConfig.with_special_ore` was enabled.

## [0.3.2] — 2026-08-24

### Changed

- Upgraded the `tracing-tracy` dependency to version 0.12.

## [0.3.1] — 2026-08-23

### Added

- Unified error type for the crate (`Error`), including the `InvalidDatabase` variant.
- Database fingerprint mechanism (`SdeFingerprint`) to detect mismatches between the
  configuration used to build the database and the one used to read it.
- `Star` struct and a color attribute integrated into `SdePoint` and into solar system
  queries.

### Changed

- Renamed the `profile-with-tracy` feature to `profile`.
- Replaced `eprintln!` calls with `tracing` throughout the crate, for consistent
  logging.
- Refactored the `typeStar` table schema and its references for consistency.

### Fixed

- Error when evaluating the sde-builder construction.

## [0.3.0] — 2026-08-23

### Changed

- Instrumented the codebase with `tracing`/`#[tracing::instrument]`.
- The `sde-builder` binary stops manually starting the Tracy client
  (`tracy_client::Client::start()` + `profiling::function_scope!()`) and instead
  installs a `tracing` subscriber with a `TracyLayer`, which starts the shared client
  automatically — migration in progress: the `profile-with-tracy` feature keeps
  feeding Tracy's backend via `profiling` for calls not yet migrated to
  `#[tracing::instrument]`.

## [0.2.2] — 2026-08-13

### Fixed

- Performance regression in `get_abstract_systems()`.

## [0.2.1] — 2026-08-13

### Added

- `profile-with-tracy` feature, replacing `puffin` for profiling.

## [0.2.0] — 2026-08-12

### Added

- Support for disallowed anchor categories/groups on solar systems, with its schema,
  queries, and tests.
- New `mapSolarSystemSubType` table and its insertion logic in the parser.
- Support for `mapAbstractSystems` (abstract regional map).
- Planet and moon data included in `get_universe()`.
- Verbose output in the sde-builder analyzer.
- Support for third-party/community data in the database build.
- Schema and analysis functions for NPC corporations, their divisions, stations, and
  station operations/services.

### Changed

- Renamed `MapPoint`/`MapSegment` to `SdePoint`/`SdeSegment`.
- Renamed the `dotlan` module to `community`.
- Reworked the `mapSolarSystems` schema: the `hub`/`corridor`/`fringe` boolean columns
  are replaced by a single `type` (TEXT) column; adds the `factionId` foreign key;
  disallowed anchor categories and groups move into separate, indexed tables.
- Removed the sde-builder GUI.

### Removed

- `kdtree` dependency (only `RTree` is kept for spatial queries).

### Fixed

- Malformed SDE record: `foreign_key_check` found unsatisfied constraints on
  `npcCorporations.stationId` against `npcStations` during sde-builder execution.

## [0.1.2] — 2026-08-07

### Added

- Entity-relationship diagram (ERD.md) and updated usage instructions in the README.
- `RTree`-based spatial query support for connection management.

### Changed

- Renamed the binaries to `sde-builder` and `sde-builder-gui`.
- SQL query formatting and readability in `SdeManager`.

## [0.1.1] — 2026-08-07

### Fixed

- Incorrect function name in `gui.rs`.

## [0.1.0] — 2026-08-07

First release of the full Rust rewrite of CCP's SDE database builder (based on the
previous Python routine), reaching functional parity with `parse_data()`.

### Added

- Full `parser` module, phase by phase: groups/types, regions/constellations, solar
  systems, stargates (`mapSystemGates`), stars and planets, moons (`mapMoons`), and
  connections (`mapSystemConnections`), each with its own `ParserConfig` settings and
  unit tests.
- `schema` module: generates the complete (`STRICT`) DDL for `sde.db`.
- `extract` module: decompresses the downloaded SDE zip, preserving the `maps/`
  directory.
- `sde_index` module: checks and conditionally downloads the current SDE build number.
- `dotlan` module: downloads and parses dotlan SVG maps; Jove Observatories data.
- Optional 2D isometric projection support (`position2DX`/`position2DY`) alongside
  CCP's precomputed value.
- Correction factor and coordinate inversion for rendering.

### Changed

- Removed the legacy `projX`/`projY`/`projZ` columns from the schema in favor of
  `position2DX`/`position2DY`.
- Coordinate types rewritten several times during the migration
  (`Coordinates2d`/`Coordinates3d` → `SystemPoint` → `MapPoint`), unifying the point
  representation.
- Comments and error messages translated from Spanish to English throughout the crate,
  for consistency.
- CI workflow reworked to support Windows builds.

## [0.0.5] — 2023-03-05

### Added

- Async loading support.
- `Clone` trait on the structs, to help with serialization.

## [0.0.4] — 2023-03-01

### Added

- Support for projection coordinates in the `SolarSystem` object.

### Changed

- Database used for unit testing.

## [0.0.3] — 2023-02-24

First version published to crates.io.

### Added

- Initial commit of the crate and crates.io packaging configuration.
