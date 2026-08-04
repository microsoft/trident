use std::path::PathBuf;

use clap::Parser;
use log::{LevelFilter, Log, Metadata, Record};

use trident_acl_agent::{
    config::{AgentConfig, GoalSource, DEFAULT_CONFIG_PATH},
    orchestrator::Orchestrator,
    run_omaha_only,
};

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

/// Harpoon can either run its original one-shot Omaha flow or the new
/// label-driven orchestrator. Activation of label mode is intentionally gated by
/// config file only: shipping defaults stay on `omaha-only`, while a VM
/// extension or AgentBaker-dropped config is expected to opt a node into the
/// AKS label protocol.
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

    /// Optional path to /etc/trident/trident-acl-agent.conf.
    #[arg(long)]
    config: Option<PathBuf>,

    /// Optional Omaha/Nebraska URL override. When omitted, Harpoon uses the
    /// endpoint from config.toml. When both are missing, startup fails with a
    /// clear error.
    #[arg()]
    url: Option<url::Url>,
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

    let config_path = args
        .config
        .clone()
        .unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG_PATH));
    let explicit_config = args.config.is_some();

    let config = AgentConfig::load(&config_path, explicit_config)?.unwrap_or_default();
    let config = config.with_cli_endpoint(args.url.clone());

    match config.orchestration.goal_source {
        GoalSource::OmahaOnly => run_omaha_only(&config).await,
        GoalSource::Labels => Orchestrator::from_config(config).await?.run().await,
    }
}
