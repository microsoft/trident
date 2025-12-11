use std::process::ExitCode;

use anyhow::Error;
use clap::Parser;
use log::{error, info};

mod cli;

use cli::Cli;
use trident_api::error::TridentError;

/// Trident version as provided by environment variables at build time
pub const TRIDENT_VERSION: &str = match option_env!("TRIDENT_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

pub mod protobufs {
    tonic::include_proto!("harpoon.v1");
}

fn run_trident_cli(_args: &Cli) -> Result<(), TridentError> {
    // Log version
    info!("Trident CLI version: {}", TRIDENT_VERSION);

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
