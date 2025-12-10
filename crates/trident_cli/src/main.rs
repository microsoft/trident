use std::{fs, path::PathBuf, process::ExitCode};

use anyhow::Error;
use clap::Parser;
use log::{error, info};

mod cli;

use cli::Cli;
use trident_api::{
    constants::{AGENT_CONFIG_PATH, TRIDENT_DATASTORE_PATH_DEFAULT},
    error::TridentError,
};

/// Trident CLI version
pub const TRIDENT_CLI_VERSION: &str = env!("CARGO_PKG_VERSION");

#[allow(unused)]
struct AgentConfig {
    datastore: PathBuf,
}

#[allow(unused)]
fn load_agent_config() -> Result<AgentConfig, TridentError> {
    let mut config = AgentConfig {
        datastore: TRIDENT_DATASTORE_PATH_DEFAULT.into(),
    };

    if let Ok(contents) = fs::read_to_string(AGENT_CONFIG_PATH) {
        for line in contents.lines() {
            if let Some(path) = line.strip_prefix("DatastorePath=") {
                config.datastore = path.trim().into();
            }
        }
    }

    Ok(config)
}

fn run_trident_cli(_args: &Cli) -> Result<(), TridentError> {
    // Log version
    info!("Trident CLI version: {}", TRIDENT_CLI_VERSION);

    // TODO: Handle CLI commands
    return Ok(());
}

fn setup_logging(args: &Cli) -> Result<(), Error> {
    env_logger::builder()
        .format_timestamp(None)
        .filter_level(args.verbosity)
        .init();

    Ok(())
}

fn main() -> ExitCode {
    // Parse args
    let args = Cli::parse();

    // Initialize the loggers
    if let Err(e) = setup_logging(&args) {
        eprintln!("Failed to initialize logging: {e:?}");
        return ExitCode::from(1);
    }

    // Run Trident CLI
    match run_trident_cli(&args) {
        Ok(()) => {
            info!("Trident CLI command completed successfully");
            ExitCode::SUCCESS
        }
        Err(e) => {
            error!("Trident CLI command failed: {e:?}");
            ExitCode::from(2)
        }
    }
}
