use std::{path::PathBuf, process::ExitCode};

use anyhow::{bail, Context, Error};
use clap::{Args, Parser, Subcommand};
use log::{error, info, LevelFilter};

use trident::{Logstream, MultiLogger};

use trident_api::error::TridentResultExt;

mod validation;

#[derive(Parser, Debug)]
#[clap(version = trident::TRIDENT_VERSION)]
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
    ParseKickstart { path: PathBuf },

    /// Validate HostConfiguration
    ///
    /// Provide one Trident Configuration file or one Host Configuration file.
    /// When no options are provided, the default Trident Configuration is
    /// validated.
    Validate {
        /// Path to a Host Configuration file
        #[clap(short = 'n', long = "host-config", conflicts_with = "config")]
        hc_path: Option<PathBuf>,
    },

    #[cfg(feature = "pytest-generator")]
    /// Generate Pytest wrappers for functional tests
    Pytest,
}

fn run_trident(mut logstream: Logstream, args: &Cli) -> Result<(), Error> {
    // Log version ASAP
    info!("Trident version: {}", trident::TRIDENT_VERSION);

    // Catch exit fast commands
    match &args.command {
        Commands::Validate { hc_path } => {
            return match (args.config.as_ref(), hc_path) {
                (Some(_), Some(_)) => bail!(
                    "Cannot validate both Trident Configuration and Host Configuration at once"
                ),
                (Some(tc_path), None) => validation::validate_trident_config_file(tc_path),
                (None, Some(hc_path)) => validation::validate_host_config_file(hc_path),
                (None, None) => {
                    validation::validate_trident_config_file(trident::TRIDENT_LOCAL_CONFIG_PATH)
                }
            }
        }
        Commands::ParseKickstart { path } => return validation::validate_setsail(path),
        #[cfg(feature = "pytest-generator")]
        Commands::Pytest => {
            pytest::generate_functional_test_manifest();
            return Ok(());
        }
        _ => (),
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

        Commands::ParseKickstart { .. } | Commands::Validate { .. } => unreachable!(),

        #[cfg(feature = "pytest-generator")]
        Commands::Pytest => unreachable!(),
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
