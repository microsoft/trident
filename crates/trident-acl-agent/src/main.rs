use anyhow::{Context, Error};
use clap::Parser;
use osutils::logging::FilteredLogger;
use systemd_journal_logger::{connected_to_journal, JournalLog};

use trident_acl_agent::{
    annotations::orchestrator::Orchestrator,
    core::config::{AgentConfig, GoalSource},
    omahaonly::run_omaha_only,
};

mod cli;
mod connection_check;

use cli::Args;
use connection_check::validate_connection;

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

#[tokio::main]
async fn main() -> Result<(), Error> {
    let args = Args::parse();

    if let Some(Ok(journal_logger)) = connected_to_journal().then(JournalLog::new) {
        let logger = FilteredLogger::new(
            journal_logger,
            args.verbosity,
            args.network_verbosity,
            NETWORK_LOG_TARGETS,
        );
        log::set_max_level(logger.max_level());
        log::set_boxed_logger(Box::new(logger))
            .map_err(Error::new)
            .context("failed to install systemd journal logger")?;
    } else {
        let inner = env_logger::builder()
            .format_timestamp(None)
            .filter_level(args.verbosity.max(args.network_verbosity))
            .build();
        let logger = FilteredLogger::new(
            inner,
            args.verbosity,
            args.network_verbosity,
            NETWORK_LOG_TARGETS,
        );
        log::set_max_level(logger.max_level());
        log::set_boxed_logger(Box::new(logger))
            .map_err(Error::new)
            .context("failed to install env logger")?;
    }

    let config = AgentConfig::from_env()?;

    if let Some(target) = args.validate_connection {
        return validate_connection(target, &config).await;
    }

    match config.orchestration.goal_source {
        // Historical one-shot flow: query Nebraska once, apply an update if
        // offered, and exit. No Kubernetes/annotation involvement. Not a
        // documented/supported deployment option (see config::GoalSource).
        GoalSource::OmahaOnly => run_omaha_only(&config).await,
        // The only supported mode: the annotation-driven reconcile loop
        // (watches <prefix>/update-request, drives stage/finalize/rollback/
        // commit against tridentd, writes <prefix>/update-status; prefix
        // defaults to acl.microsoft.com, overridable via
        // TRIDENT_ACL_AGENT_KUBERNETES_ANNOTATION_PREFIX).
        GoalSource::Annotations => Orchestrator::from_config(config).await?.run().await,
    }
}
