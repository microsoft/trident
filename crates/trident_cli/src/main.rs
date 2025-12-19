use std::process::ExitCode;

use anyhow::Error;
use clap::Parser;
use log::{error, info};

pub mod cli;
pub mod client;

use cli::Cli;
use trident_api::error::TridentError as ApiTridentError;

// Include generated gRPC code
tonic::include_proto!("harpoon.v1");

/// Trident version as provided by environment variables at build time
pub const TRIDENT_VERSION: &str = match option_env!("TRIDENT_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

#[tokio::main]
async fn run_trident_cli(args: &Cli) -> Result<(), ApiTridentError> {
    // Log version
    info!("Trident CLI version: {}", TRIDENT_VERSION);

    // Create gRPC client
    let mut client = client::TridentClient::new("http://localhost:3322")
        .await
        .map_err(|e| {
            ApiTridentError::with_source(
                trident_api::error::InternalError::Internal("Failed to create gRPC client"),
                e,
            )
        })?;

    // Handle CLI commands
    client.handle_command(&args.command).await.map_err(|e| {
        ApiTridentError::with_source(
            trident_api::error::InternalError::Internal("Command failed"),
            e,
        )
    })?;

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
