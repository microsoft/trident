use std::{
    path::{Path, PathBuf},
    process::ExitCode,
};

use anyhow::{bail, Context, Error};
use clap::{Args, Parser, Subcommand};
use log::{error, info, LevelFilter};

use trident::{Logstream, MultiLogger};

use setsail::KsTranslator;
use trident_api::error::TridentResultExt;

/// Trident version as provided by environment variables at build time
pub const TRIDENT_VERSION: &str = match option_env!("TRIDENT_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

#[derive(Parser, Debug)]
#[clap(version = TRIDENT_VERSION)]
struct Cli {
    /// Path to the Trident Configuration file
    #[clap(global = true, short, long)]
    config: Option<PathBuf>,

    /// Logging verbosity [OFF, ERROR, WARN, INFO, DEBUG, TRACE]
    #[arg(global = true, short, long, default_value_t = LevelFilter::Warn)]
    verbosity: LevelFilter,

    #[clap(subcommand)]
    command: Commands,
}

#[derive(Args, Debug)]
struct GetArgs {
    /// Path to save the resulting HostStatus
    #[clap(short, long)]
    status: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Apply the HostConfiguration
    Run(GetArgs),

    /// Configure OS networking based on Trident Configuration
    #[clap(name = "start-network")]
    StartNetwork,

    /// Get the HostStatus
    #[clap(name = "get")]
    GetHostStatus(GetArgs),

    /// Validates input KickStart file
    // TODO(5910): Remove this in the future
    ParseKickstart { file: String },
}

fn run_trident(mut logstream: Logstream, args: &Cli) -> Result<(), Error> {
    // Log version ASAP
    info!("Trident version: {}", TRIDENT_VERSION);

    // TODO(5910): Remove this in the future
    if let Commands::ParseKickstart { ref file } = args.command {
        let translator = KsTranslator::new().include_fail_is_error(false);
        match translator.translate(
            setsail::load_kickstart_file(Path::new(file))
                .context(format!("Failed to read {file}"))?,
        ) {
            Ok(hc) => {
                println!("{}", serde_yaml::to_string(&hc)?);
                return Ok(());
            }
            Err(e) => {
                error!(
                    "Failed to translate kickstart:\n{}",
                    serde_json::to_string_pretty(&e.0)?
                );
                bail!("Failed to translate kickstart");
            }
        };
    }

    // Lock the logstream if we're starting the network
    // We have no network yet, so we can't send logs anywhere
    if let Commands::StartNetwork = args.command {
        logstream.disable();
    }

    let mut trident = trident::Trident::new(args.config.clone(), logstream)
        .unstructured("Failed to initialize trident")?;

    match &args.command {
        Commands::Run(args) => {
            // Log version again so we can see it in the logstream
            info!("Running Trident version: {}", TRIDENT_VERSION);
            let res = trident
                .run()
                .unstructured("Failed to execute Trident run command");

            // return HostStatus if requested
            if args.status.is_some() {
                if let Err(e) = trident
                    .retrieve_host_status(&args.status)
                    .context("Failed to retrieve Host Status")
                {
                    error!("{e}");
                }
            }

            res?;
        }
        Commands::StartNetwork => trident
            .start_network()
            .unstructured("Failed to start network")?,
        Commands::GetHostStatus(args) => trident
            .retrieve_host_status(&args.status)
            .context("Failed to retrieve Host Status")?,

        // TODO(5910): Remove this in the future
        Commands::ParseKickstart { .. } => unreachable!(),
    }

    Ok(())
}

fn setup_logging(args: &Cli) -> Result<Logstream, Error> {
    let logstream = Logstream::create();

    // Set up the multilogger
    MultiLogger::new()
        .with_max_level(args.verbosity)
        .with_logger(Box::new(
            env_logger::builder()
                .format_timestamp(None)
                .filter_level(args.verbosity)
                .build(),
        ))
        .with_logger(logstream.make_logger())
        .init()
        .expect("Logger already registered");

    Ok(logstream)
}

fn main() -> ExitCode {
    // Parse args
    let args = Cli::parse();

    // Initialize the loggers
    let logstream = setup_logging(&args);
    if let Err(e) = logstream {
        error!("Failed to initialize logging: {e:?}");
        return ExitCode::from(1);
    }

    // Invoke Trident
    if let Err(e) = run_trident(logstream.unwrap(), &args) {
        error!("Trident failed: {e:?}");
        return ExitCode::from(2);
    }

    ExitCode::SUCCESS
}
