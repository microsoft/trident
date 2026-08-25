use clap::Parser;
use log::LevelFilter;

/// trident-acl-agent can either run the annotation-driven orchestrator (the
/// default) or fall back to its original one-shot Omaha flow. Mode selection
/// is environment-variable only (`TRIDENT_ACL_AGENT_ORCHESTRATION_GOAL_SOURCE`):
/// shipping defaults enable the AKS annotation protocol, while a VM
/// extension, systemd drop-in, or AgentBaker-set environment can opt a node
/// out to `omaha-only` if needed.
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub struct Args {
    /// Logging verbosity [OFF, ERROR, WARN, INFO, DEBUG, TRACE]
    #[arg(global = true, short, long, default_value_t = LevelFilter::Debug)]
    pub verbosity: LevelFilter,

    /// Logging verbosity for the underlying HTTP/gRPC/watch client stack
    /// (hyper, h2, tower, tonic, reqwest, rustls, kube). Kept separate from
    /// `--verbosity` because it can be extremely noisy (per-frame HTTP2
    /// detail, watch reconnect churn) [OFF, ERROR, WARN, INFO, DEBUG, TRACE].
    #[arg(global = true, long, default_value_t = LevelFilter::Warn)]
    pub network_verbosity: LevelFilter,

    /// Validate connectivity to a single dependency and exit immediately,
    /// instead of running the agent. Useful for troubleshooting one
    /// connection in isolation (e.g. a systemd ExecStartPre check, or manual
    /// diagnostics on-node) without running the full orchestrator loop.
    /// Exits with status 0 if the connection could be established, non-zero
    /// (with an error message) otherwise.
    #[arg(long, value_enum)]
    pub validate_connection: Option<ConnectionTarget>,
}

/// A single dependency `--validate-connection` can check.
#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum ConnectionTarget {
    /// Validates reachability of the Kubernetes API server by fetching this
    /// node's own Node object (the same access the agent's reconcile loop
    /// already requires).
    Kubernetes,
    /// Validates reachability of tridentd by connecting to its gRPC Unix
    /// socket. Connecting is sufficient - no RPC call is needed, since the
    /// connection itself fails immediately if nothing is listening.
    Tridentd,
    /// Validates reachability of the Nebraska/Omaha server by issuing a real
    /// update-check query. Any well-formed Omaha response (including "no
    /// update available") counts as success - only a network/transport
    /// failure is treated as unreachable.
    Nebraska,
}
