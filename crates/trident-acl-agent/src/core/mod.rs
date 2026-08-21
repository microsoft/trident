//! Shared building blocks used by both the annotation-driven protocol
//! ([`crate::annotations`]) and the legacy one-shot Omaha flow
//! ([`crate::omahaonly`]): configuration, error types, machine-id resolution,
//! the current-version fallback, the `tridentd` gRPC client, and the generic
//! Nebraska/Omaha protocol client.

pub mod config;
pub mod error;
pub mod id;
pub mod nebraska;
pub mod trident;
pub mod version;
