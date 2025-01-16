//! # Storage Graph & Builder
//!
//! The purpose of this module is to build a graph of all storage entities and
//! their relationships. A big part of the building process is checking that the
//! graph is valid according to predefined rules.
//!
//! In broad terms, this module is used as follows:
//!
//! 1. Create a `StorageGraphBuilder` instance.
//! 2. Feed it nodes by converting Host Config Storage objects (eg. `Disk`s,
//!    `Partition`s...) into nodes (`StorageGraphNode`).
//!    - This is done with the `From` traits defined in `conversions.rs` and
//!      passing the nodes to the builder with `add_node()`.
//! 3. Call `build()` to get a `StorageGraph` instance.
//! 4. On success, a valid `StorageGraph` instance is returned. Otherwise, an
//!    error detailing the issue is returned.
//!
//! Generic rules, such as checking for duplicate IDs, are implemented in the
//! building itself (builder module). Rules and constraints related to specific
//! node types are placed in the `rules` module.
//!
//! ## Layout
//!
//! ```text
//! trident_api/src/config/host/storage/storage_graph
//! ├── builder --------------> # Graph builder module.
//! |   ├── mod.rs -----------> # StorageGraphBuilder & core building logic.
//! │   └── ... --------------> # Submodules for specific building steps.
//! ├── cardinality.rs -------> # Helper checking cardinality rules.
//! ├── containers.rs --------> # Helper containers for rules and data.
//! ├── conversions.rs -------> # From traits for converting Host Config Storage objects into graph objects.
//! ├── errors.rs ------------> # Error types.
//! ├── graph.rs -------------> # StorageGraph.
//! ├── mod.rs ---------------> # This file.
//! ├── node.rs --------------> # StorageGraphNode & associated logic/types.
//! ├── references.rs --------> # Structs to describe references, their types, and the logic associated with them.
//! ├── rules ----------------> # Rules for validating the graph.
//! │   └── mod.rs -----------> # Rules module.
//! ├── types.rs -------------> # Types used by the graph.
//! └── validation_tests.rs --> # Validation tests.
//! ```
//!

// Modules directly related to the graph and its building.
pub(super) mod builder;
pub(super) mod conversions;
pub mod graph;
pub(super) mod node;
pub mod references;
pub mod types;

// Rules & rule helpers.
// Public so docbuilder can access these.
pub mod cardinality;
pub mod containers;
pub mod rules;

// Implementations of fmt::Display for the types in this module.
pub mod display;

// Errors module.
// Public because it's used by trident to report errors.
pub mod error;

// Validation unit tests.
#[cfg(test)]
mod validation_tests;
