//! # Block Device Graph & Builder
//!
//! The purpose of this module is to build a graph of block devices and their
//! relationships. A big part of the building process is validating that the
//! graph is valid.
//!
//! In broad terms, this module is used as follows:
//!
//! 1. Create a `BlockDeviceGraphBuilder` instance.
//! 2. Feed it nodes by converting Host Config Storage objects (eg. `Disk`s,
//!    `Partition`s...) into nodes (`BlkDevNode`).
//!    - This is done with the `From` traits defined in `conversions.rs` and
//!      passing the nodes to the builder with `add_node()`.
//! 3. Call `build()` to get a `BlockDeviceGraph` instance.
//! 4. On success, a valid `BlockDeviceGraph` instance is returned. Otherwise,
//!    an error detailing the issue is returned.
//!
//! Generic rules, such as checking for duplicate IDs, are implemented in the
//! building itself (`builder.rs`). Rules and constrains related to specific
//! node types are placed in the `rules` module.
//!
//! ## Layout
//!
//! ```text
//! trident_api/src/config/host/storage/blkdev_graph
//! ├── builder.rs        # BlockDeviceGraphBuilder & building logic
//! ├── cardinality.rs    # Helper checking cardinality rules
//! ├── conversions.rs    # From traits for converting Host Config Storage objects into graph objects
//! ├── errors.rs         # Error types
//! ├── graph.rs          # BlockDeviceGraph
//! ├── mod.rs            # This file
//! ├── mountpoints.rs    # Helpers for checking mountpoint validity rules
//! ├── partitions.rs     # Helpers for checking partition rules and finding partition information
//! ├── rules             # Rules for validating the graph
//! │   └── mod.rs        # Rules module
//! ├── types.rs          # Types used by the graph
//! └── validation_tests.rs
//! ```
//!

pub(super) mod builder;
pub(super) mod cardinality;
pub(super) mod conversions;
pub mod error;
pub(super) mod graph;
pub(super) mod mountpoints;
pub(super) mod partitions;
pub(super) mod rules;
pub(super) mod types;

#[cfg(test)]
mod validation_tests;
