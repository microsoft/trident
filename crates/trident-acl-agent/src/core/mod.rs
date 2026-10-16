//! Building blocks shared by both the annotation-driven and `omaha-only`
//! modes: env-var config loading ([`config`]), COSI metadata SHA-384
//! resolution ([`cosi_sha384`]), the crate's unified error type
//! ([`error`]), machine-id derivation ([`id`]), current-version detection
//! ([`version`]), the `tridentd` gRPC client ([`trident`]), and the
//! Nebraska/Omaha protocol client ([`nebraska`]).

pub mod config;
pub mod cosi_sha384;
pub mod error;
pub mod id;
pub mod nebraska;
pub mod trident;
pub mod version;
