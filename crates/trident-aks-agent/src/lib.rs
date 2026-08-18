//! # trident-aks-agent
//!
//! AKS-RP-facing sidecar that watches this node's own Kubernetes
//! `acl.azure.com/update-request` annotation and drives Trident's
//! stage/finalize/rollback/commit operations against `tridentd` over gRPC,
//! writing progress back to `acl.azure.com/update-status` and
//! `acl.azure.com/update-commit-status`. Ships with a systemd unit
//! (`trident-aks-agent.service`) and is configured entirely via
//! `TRIDENT_AKS_AGENT_*` environment variables (see [`config`]).
//!
//! Historically this logic (and the standalone one-shot Omaha flow it grew
//! out of) lived together in a single `trident-acl-agent` binary
//! (internally codenamed "Harpoon", a name no longer used). They have since
//! been split into two binaries:
//! this crate (the annotation-driven orchestrator) and `trident-acl-agent`
//! (the one-shot Omaha-only flow, now with no Kubernetes dependency at
//! all). Shared plumbing - the Nebraska/Omaha client, the tridentd gRPC
//! client, and machine-id helpers - lives in `trident-agent-core`, which
//! both binaries link.

pub mod annotations;
pub mod config;
pub mod k8s;
pub mod orchestrator;
pub mod state;

/// Only built for `cargo test` (relies on trident-proto's `server` feature,
/// which is only enabled via trident-aks-agent's dev-dependencies - see
/// mock_tridentd.rs's module docs).
#[cfg(test)]
pub mod mock_tridentd;
