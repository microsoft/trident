//! Practical validation harness for the Harpoon AKS label protocol.
//!
//! This crate implements the tooling described in the design doc's validation
//! section (§13): a fake apiserver, RP scenario runner, fake kubelet, Nebraska
//! proxy, and a reboot-interception shim.

pub mod apiserver;
pub mod kubelet;
pub mod nebraska;
pub mod rp;
pub mod scenario;
