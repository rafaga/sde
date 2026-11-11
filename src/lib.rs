#![crate_name = "sde"]
//! Read Eve Online's SDE data from sqlite database
//!
//! Provides an abstraction layer over SDE data .
//! When the abstraction is used makes it fast to search
//! there are these advantages:
//!
//!

/// Crate-wide error type. See `src/error.rs` for the detail on why
/// `SdeManager`'s read methods and, with the `builder` feature,
/// everything under `builder` all share this one type now.
mod error;
pub use error::Error;

/// Module that has Data object abstractions to fill with the database data.
pub mod objects;

/// Route planning over the stargate network: nodes are gates, so the
/// distance flown inside each system is part of the cost.
pub mod gate_graph;

/// Logic to (re)generate `sde.db` (feature `builder`, disabled by
/// default). See `src/builder/mod.rs` for the detail.
#[cfg(feature = "builder")]
pub mod builder;

/// Moving apart the systems the 2D projection puts on top of each other,
/// as a postprocessing step of the finished database (feature
/// `projection-patch`, enabled by default). See `src/projection_patch.rs`.
#[cfg(feature = "projection-patch")]
pub mod projection_patch;

/// Connections between systems that are not stargates (wormholes), kept in
/// a layer next to the universe. See `src/links.rs`.
pub mod links;

/// Where to put an extra node next to a system without it touching another:
/// a free place near the system, in plain coordinates. See
/// `src/placement.rs`.
pub mod placement;

pub use objects::SdeManager;
