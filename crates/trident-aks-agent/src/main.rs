use anyhow::Context;
use clap::Parser;
use log::{LevelFilter, Log, Metadata, Record};

use trident_agent_core::{check_nebraska_reachable, trident::TridentClient, IdSource};
use trident_aks_agent::{config::AgentConfig, k8s::NodeClient, orchestrator::Orchestrator};

/// Module/target prefixes for the underlying HTTP/gRPC/watch client stack.
/// These crates emit very verbose `log`-facade tracing (connection setup,
/// per-frame HTTP2 detail, watch reconnect churn) that is rarely useful at
/// the same verbosity as the agent's own orchestration logic, so it's
/// filtered independently via `--network-verbosity`.
const NETWORK_LOG_TARGETS: &[&str] = &[
    "hyper",
    "h2",
    "tower",
    "tonic",
    "reqwest",
    "rustls",
    "kube",
    "kube_client",
    "kube_runtime",
];

/// A `log::Log` wrapper that applies a separate level filter to the noisy
/// HTTP/gRPC/watch client crates (see [`NETWORK_LOG_TARGETS`]) while leaving
/// every other target (the agent's own code) at the main `--verbosity`
/// level.
struct FilteredLogger<L> {
    inner: L,
    verbosity: LevelFilter,
    network_verbosity: LevelFilter,
}

impl<L: Log> Log for FilteredLogger<L> {
    fn enabled(&self, metadata: &Metadata) -> bool {
        let level = if is_network_target(metadata.target()) {
            self.network_verbosity
        } else {
            self.verbosity
        };
        metadata.level() <= level
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            self.inner.log(record);
        }
    }

    fn flush(&self) {
        self.inner.flush();
    }
}

fn is_network_target(target: &str) -> bool {
    NETWORK_LOG_TARGETS
        .iter()
        .any(|prefix| target == *prefix || target.starts_with(&format!("{prefix}::")))
}

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Logging verbosity [OFF, ERROR, WARN, INFO, DEBUG, TRACE]
    #[arg(global = true, short, long, default_value_t = LevelFilter::Debug)]
    verbosity: LevelFilter,

    /// Logging verbosity for the underlying HTTP/gRPC/watch client stack
    /// (hyper, h2, tower, tonic, reqwest, rustls, kube). Kept separate from
    /// `--verbosity` because it can be extremely noisy (per-frame HTTP2
    /// detail, watch reconnect churn) [OFF, ERROR, WARN, INFO, DEBUG, TRACE].
    #[arg(global = true, long, default_value_t = LevelFilter::Warn)]
    network_verbosity: LevelFilter,

    /// Validate connectivity to a single dependency and exit immediately,
    /// instead of running the agent. Useful for troubleshooting one
    /// connection in isolation (e.g. a systemd ExecStartPre check, or manual
    /// diagnostics on-node) without running the full orchestrator loop.
    /// Exits with status 0 if the connection could be established, non-zero
    /// (with an error message) otherwise.
    #[arg(long, value_enum)]
    validate_connection: Option<ConnectionTarget>,
}

/// A single dependency `--validate-connection` can check.
#[derive(Clone, Copy, Debug, clap::ValueEnum)]
enum ConnectionTarget {
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

/// Checks connectivity to exactly one of `target`'s dependencies and returns
/// `Ok(())` on success. The caller (`main`) surfaces any `Err` the normal way
/// (`anyhow`'s `Termination` impl prints the error and exits non-zero), so
/// this function only needs to produce a descriptive error on failure - no
/// explicit `process::exit` is required.
async fn validate_connection(
    target: ConnectionTarget,
    config: &AgentConfig,
) -> Result<(), anyhow::Error> {
    match target {
        ConnectionTarget::Kubernetes => {
            let client = NodeClient::new(&config.kubernetes)
                .await
                .context("failed to build Kubernetes client")?;
            // Report the actually-resolved server (kubeconfig's own server,
            // unless overridden by kubernetes.api_server), not a value
            // guessed from config - the two only match when an override is
            // set.
            let cluster_url = client.cluster_url();
            client
                .get_node(&config.kubernetes.node_name)
                .await
                .with_context(|| {
                    format!(
                        "failed to reach Kubernetes API server at {} (get Node {:?})",
                        cluster_url, config.kubernetes.node_name
                    )
                })?;
            log::info!(
                "kubernetes: reached API server at {} and fetched Node {:?}",
                cluster_url,
                config.kubernetes.node_name
            );
        }
        ConnectionTarget::Tridentd => {
            TridentClient::connect(&config.trident.socket)
                .await
                .with_context(|| {
                    format!("failed to reach tridentd at {}", config.trident.socket)
                })?;
            log::info!("tridentd: connected to {}", config.trident.socket);
        }
        ConnectionTarget::Nebraska => {
            let endpoint = config.nebraska.endpoint.clone().ok_or_else(|| {
                anyhow::anyhow!(
                    "nebraska.endpoint is not configured (set TRIDENT_AKS_AGENT_NEBRASKA_ENDPOINT)"
                )
            })?;
            let app_id = config.nebraska.app_id.clone();
            // check_nebraska_reachable() is a blocking call (reqwest::blocking
            // under the hood) - calling it directly from this async fn can
            // panic ("Cannot drop a runtime in a context where blocking is
            // not allowed") because reqwest::blocking spins up its own inner
            // Tokio runtime per call, which isn't safe to tear down from
            // inside an already-running async task. Run it on a dedicated
            // blocking thread instead.
            //
            // Deliberately uses check_nebraska_reachable() rather than
            // query_for_update(): the latter also validates app-level
            // semantics (app ID match, non-error status), which would make
            // this a "can we get a valid update check" test rather than the
            // pure reachability check documented on ConnectionTarget::Nebraska
            // above.
            let endpoint_for_task = endpoint.clone();
            let track = config.nebraska.track.clone();
            tokio::task::spawn_blocking(move || {
                check_nebraska_reachable(
                    &endpoint_for_task,
                    &app_id,
                    &track,
                    IdSource::MachineIdHashed,
                )
            })
            .await
            .context("Nebraska connectivity check task panicked")?
            .with_context(|| format!("failed to reach Nebraska server at {endpoint}"))?;
            log::info!("nebraska: reached server at {endpoint}");
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    let args = Args::parse();

    let max_level = args.verbosity.max(args.network_verbosity);
    if let Some(Ok(journal_logger)) =
        systemd_journal_logger::connected_to_journal().then(systemd_journal_logger::JournalLog::new)
    {
        log::set_boxed_logger(Box::new(FilteredLogger {
            inner: journal_logger,
            verbosity: args.verbosity,
            network_verbosity: args.network_verbosity,
        }))
        .expect("Failed to install systemd journal logger");
        log::set_max_level(max_level);
    } else {
        let inner = env_logger::builder()
            .format_timestamp(None)
            .filter_level(max_level)
            .build();
        log::set_boxed_logger(Box::new(FilteredLogger {
            inner,
            verbosity: args.verbosity,
            network_verbosity: args.network_verbosity,
        }))
        .expect("Failed to install env logger");
        log::set_max_level(max_level);
    }

    let config = AgentConfig::from_env()?;

    if let Some(target) = args.validate_connection {
        return validate_connection(target, &config).await;
    }

    Orchestrator::from_config(config).await?.run().await
}
