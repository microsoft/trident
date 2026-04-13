pub mod config;
pub mod constants;
pub mod error;
pub mod misc;
pub mod primitives;
pub mod status;

#[cfg(feature = "schemars")]
pub mod schema;

/// Identifier for a block device. Needs to be unique across all types of devices.
pub type BlockDeviceId = String;

/// Returns true if the given value is equal to its default value.
/// Useful for #[serde(skip_serializing_if = "default")]
pub fn is_default<T: Default + PartialEq>(t: &T) -> bool {
    *t == Default::default()
}

/// The samples module contains sample data for the API.
///
/// The samples are only used in the documentation. Therefore it is gated by a feature flag.
#[cfg(feature = "samples")]
pub mod samples;

/// The storage graph submodule.
pub use config::host::storage::storage_graph;

/// Re-export dependency so docbuilder can use the exact same version without having to manage a
/// separate dependency in docbuilder's Cargo.toml.
#[cfg(feature = "schemars")]
pub use schemars;
