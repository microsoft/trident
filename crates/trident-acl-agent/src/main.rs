use anyhow::{Context, Error};
use clap::Parser;
use osutils::logging::filter::LogFilter;
use systemd_journal_logger::{connected_to_journal, JournalLog};

use trident_acl_agent::{
    annotations::orchestrator::Orchestrator,
    core::config::{AgentConfig, Mode},
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

/// Wraps `inner` in a [`LogFilter`] that caps overall verbosity at
/// `args.verbosity`, then further caps every [`NETWORK_LOG_TARGETS`] prefix
/// down to `args.network_verbosity` (matching osutils::logging's shared
/// `LogFilter`/`MultiLogger` pattern already used by `trident`'s own
/// `main.rs`, rather than a bespoke wrapper type).
fn build_logger<L: log::Log>(inner: L, args: &Args) -> LogFilter<L> {
    NETWORK_LOG_TARGETS.iter().fold(
        LogFilter::new(inner).with_max_level(args.verbosity),
        |logger, target| logger.with_global_filter(*target, args.network_verbosity),
    )
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let args = Args::parse();

    // LogFilter has no single accessor for "the loosest level anything could
    // be logged at" (unlike the old FilteredLogger::max_level()), so compute
    // it the same way FilteredLogger did: the looser of the two configured
    // verbosities, so the `log` facade doesn't drop records before this
    // filter gets a chance to apply the per-target ceiling.
    let max_level = args.verbosity.max(args.network_verbosity);

    if let Some(Ok(journal_logger)) = connected_to_journal().then(JournalLog::new) {
        let logger = build_logger(journal_logger, &args);
        log::set_max_level(max_level);
        log::set_boxed_logger(Box::new(logger))
            .map_err(Error::new)
            .context("failed to install systemd journal logger")?;
    } else {
        let inner = env_logger::builder()
            .format_timestamp(None)
            .filter_level(max_level)
            .build();
        let logger = build_logger(inner, &args);
        log::set_max_level(max_level);
        log::set_boxed_logger(Box::new(logger))
            .map_err(Error::new)
            .context("failed to install env logger")?;
    }

    let config = AgentConfig::from_env()?;

    if let Some(target) = args.validate_connection {
        return validate_connection(target, &config).await;
    }

    match config.orchestration.mode {
        // Historical one-shot flow: query Nebraska once, apply an update if
        // offered, and exit. No Kubernetes/annotation involvement. Not a
        // documented/supported deployment option (see config::Mode).
        Mode::OmahaOnly => run_omaha_only(&config).await,
        // The only supported mode: the annotation-driven reconcile loop
        // (watches <prefix>/update-request, drives stage/finalize/rollback/
        // commit against tridentd, writes <prefix>/update-status; prefix
        // defaults to acl.microsoft.com, overridable via
        // TRIDENT_ACL_AGENT_KUBERNETES_ANNOTATION_PREFIX).
        Mode::Annotations => Orchestrator::from_config(config).await?.run().await,
    }
}
