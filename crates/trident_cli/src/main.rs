use std::process::ExitCode;

use anyhow::Error;
use clap::Parser;
use log::{error, info};

mod cli;

use cli::Cli;
use trident_api::error::TridentError;

/// Trident CLI version
pub const TRIDENT_CLI_VERSION: &str = env!("CARGO_PKG_VERSION");

fn run_trident_cli(_args: &Cli) -> Result<(), TridentError> {
    // Log version
    info!("Trident CLI version: {}", TRIDENT_CLI_VERSION);

    // TODO: Handle CLI commands
    Ok(())
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
