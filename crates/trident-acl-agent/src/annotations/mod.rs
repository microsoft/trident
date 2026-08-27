//! The default Kubernetes annotation-driven update protocol.
//!
//! - [`protocol`] defines the annotation schema (`UpdateRequest`/
//!   `UpdateStatus`/`StatusCode`).
//! - [`k8s`] is the Kubernetes Node get/watch/patch client.
//! - [`state`] persists in-flight/completed operations across the reboot
//!   that finalize triggers.
//! - [`orchestrator`] is the reconcile loop tying them all together.

mod protocol;
pub use protocol::*;

pub mod k8s;
pub mod orchestrator;
pub mod state;
