use std::path::PathBuf;

use clap::Parser;
use log::LevelFilter;

use trident_acl_agent::{
    config::{AgentConfig, GoalSource, DEFAULT_CONFIG_PATH},
    orchestrator::Orchestrator,
    run_omaha_only,
};

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

    if let Some(Ok(journal_logger)) =
        systemd_journal_logger::connected_to_journal().then(systemd_journal_logger::JournalLog::new)
    {
        journal_logger
            .install()
            .expect("Failed to install systemd journal logger");
        log::set_max_level(args.verbosity);
    } else {
        env_logger::builder()
            .format_timestamp(None)
            .filter_level(args.verbosity)
            .init();
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
