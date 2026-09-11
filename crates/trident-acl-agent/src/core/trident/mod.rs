//! gRPC client for talking to `tridentd`, plus its in-process mock used only
//! by unit tests.

mod client;
pub use client::*;

#[cfg(test)]
pub mod mock;
