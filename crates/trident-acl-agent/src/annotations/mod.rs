//! The Kubernetes annotation-driven update protocol - the crate's default
//! mode (see [`crate::core::config::GoalSource`]).
//!
//! [`protocol`] defines the annotation schema (request/status types, keys,
//! schema version); [`k8s`] is the Kubernetes Node get/watch/patch client;
//! [`state`] persists in-flight/completed operations across the reboot that
//! finalize triggers; [`orchestrator`] is the reconcile loop tying them all
//! together.

mod protocol;
pub use protocol::*;

pub mod k8s;
pub mod orchestrator;
pub mod state;
