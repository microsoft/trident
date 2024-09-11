use std::{panic, path::PathBuf, process::ExitCode};

use anyhow::{bail, Context, Error};
use clap::{Args, Parser, Subcommand};
use log::{error, info, LevelFilter};

use trident::{offline_init, BackgroundLog, Logstream, MultiLogger, TraceStream};
use trident_api::error::TridentResultExt;

mod validation;

#[derive(Parser, Debug)]
#[clap(version = trident::TRIDENT_VERSION)]
struct Cli {
    /// Path to the Trident Configuration file
    #[clap(global = true, short, long)]
    config: Option<PathBuf>,

    /// Logging verbosity [OFF, ERROR, WARN, INFO, DEBUG, TRACE]
    #[arg(global = true, short, long, default_value_t = LevelFilter::Info)]
    verbosity: LevelFilter,

    #[clap(subcommand)]
    command: Commands,
}

#[derive(Args, Debug)]
struct GetArgs {
    /// Path to save the resulting HostStatus
    #[clap(short, long)]
    status: Option<PathBuf>,

    /// Path to save an eventual fatal error
    #[clap(short, long)]
    error: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Apply the HostConfiguration
    Run(GetArgs),

    /// Rebuild software RAID arrays managed by Trident
    #[clap(name = "rebuild-raid")]
    RebuildRaid(GetArgs),

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

    /// Initialize Trident in offline mode
    OfflineInitialize {
        /// Path to a Host Status file
        hs_path: PathBuf,
    },
}

fn run_trident(
    mut logstream: Logstream,
    mut tracestream: TraceStream,
    args: &Cli,
) -> Result<(), Error> {
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

        Commands::OfflineInitialize { hs_path } => {
            return offline_init::execute(hs_path)
                .unstructured("Failed to offline initialize Trident datastore")
        }

        _ => (),
    }

    // Lock the streams if we're starting the network
    // We have no network yet, so we can't send logs or traces anywhere
    if let Commands::StartNetwork = args.command {
        logstream.disable();
        tracestream.disable();
    }

    let res = panic::catch_unwind(move || {
        let mut trident = trident::Trident::new(args.config.clone(), logstream, tracestream)
            .unstructured("Failed to initialize trident")?;

        // After initialization, create a trace event for the purpose of
        // measuring Trident reboot times
        tracing::info!(metric_name = "trident_start");

        match &args.command {
            Commands::Run(args) => {
                let res = trident.run();

                // return HostStatus if requested
                if args.status.is_some() {
                    if let Err(e) = trident
                        .retrieve_host_status(&args.status)
                        .context("Failed to retrieve Host Status")
                    {
                        error!("{e}");
                    }
                }

                // return error if requested
                if let Some(error_path) = args.error.as_ref() {
                    if let Err(e) = &res {
                        // error fails to serialize, tracked by https://dev.azure.com/mariner-org/ECF/_workitems/edit/7420/
                        if let Err(e2) = std::fs::write(
                            error_path,
                            serde_yaml::to_string(&e).unwrap_or("".into()),
                        ) {
                            error!("Failed to write error to file: {e2}");
                        }
                    }
                }

                res.unstructured("Failed to execute Trident run command")?;
            }
            Commands::StartNetwork => trident
                .start_network()
                .unstructured("Failed to start network")?,
            Commands::GetHostStatus(args) => trident
                .retrieve_host_status(&args.status)
                .context("Failed to retrieve Host Status")?,

            Commands::ParseKickstart { .. }
            | Commands::Validate { .. }
            | Commands::OfflineInitialize { .. } => unreachable!(),

            #[cfg(feature = "pytest-generator")]
            Commands::Pytest => unreachable!(),

            Commands::RebuildRaid(args) => {
                let res = trident.rebuild_raid();
                // return HostStatus if requested
                if args.status.is_some() {
                    if let Err(e) = trident
                        .retrieve_host_status(&args.status)
                        .context("Failed to retrieve Host Status")
                    {
                        error!("{e}");
                    }
                }
                res.unstructured("Failed to execute Trident rebuild-raid command")?;
            }
        }

        Ok(())
    });

    match res {
        Err(e) => bail!("Trident panicked: {e:?}"),
        Ok(r) => r,
    }
}

fn setup_logging(args: &Cli) -> Result<Logstream, Error> {
    let logstream = Logstream::create();

    // Set up the multilogger
    let mut multilogger = MultiLogger::new()
        .with_logger(Box::new(
            env_logger::builder()
                .format_timestamp(None)
                .filter_level(args.verbosity)
                .build(),
        ))
        .with_logger(logstream.make_logger_with_level(LevelFilter::Trace));

    if matches!(args.command, Commands::Run(_)) || matches!(args.command, Commands::RebuildRaid(_))
    {
        multilogger
            .add_logger(BackgroundLog::new(trident::TRIDENT_BACKGROUND_LOG_PATH).into_logger());
    }

    multilogger.init().context("Logger already registered")?;

    Ok(logstream)
}

fn setup_tracing() -> Result<TraceStream, Error> {
    use tracing_subscriber::{filter, layer::SubscriberExt, Layer};

    let tracestream = TraceStream::default();
    // Set up the trace sender
    let trace_sender = tracestream
        .make_trace_sender()
        .with_filter(filter::LevelFilter::INFO);

    tracing::subscriber::set_global_default(
        tracing_subscriber::Registry::default().with(trace_sender),
    )
    .context("Failed to set global default subscriber")?;

    Ok(tracestream)
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

    // Initialize the telemetry flow
    let tracestream = setup_tracing();
    if let Err(e) = tracestream {
        error!("Failed to initialize tracing: {e:?}");
        return ExitCode::from(1);
    }

    // Invoke Trident
    if let Err(e) = run_trident(logstream.unwrap(), tracestream.unwrap(), &args) {
        error!("Trident failed: {e:?}");
        return ExitCode::from(2);
    }

    ExitCode::SUCCESS
}
