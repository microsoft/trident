use std::{
    fmt::{Display, Formatter, Result as FmtResult},
    panic,
    path::PathBuf,
    process::ExitCode,
};

use anyhow::{Context, Error};
use clap::{Parser, Subcommand};
use log::{error, info, warn, LevelFilter};

use trident::{
    offline_init, validation, BackgroundLog, Logstream, MultiLogger, TraceStream, Trident,
    TRIDENT_BACKGROUND_LOG_PATH,
};
use trident_api::{
    config::{HostConfigurationSource, Operation, Operations},
    constants::{AGENT_CONFIG_PATH, TRIDENT_DATASTORE_PATH_DEFAULT},
    error::{InternalError, TridentError, TridentResultExt},
};

#[derive(Parser, Debug)]
#[clap(version = trident::TRIDENT_VERSION)]
struct Cli {
    /// Logging verbosity [OFF, ERROR, WARN, INFO, DEBUG, TRACE]
    #[arg(global = true, short, long, default_value_t = LevelFilter::Info)]
    verbosity: LevelFilter,

    #[clap(subcommand)]
    command: Commands,
}

#[derive(clap::ValueEnum, Clone, Debug, Eq, PartialEq)]
enum AllowedOperation {
    Stage,
    Finalize,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Apply the HostConfiguration
    Run {
        /// The new configuration to apply
        #[clap(short, long, default_value = "/etc/trident/config.yaml")]
        config: PathBuf,

        #[clap(long, value_delimiter = ',', num_args = 0.., default_value = "stage,finalize")]
        allowed_operations: Vec<AllowedOperation>,

        /// Path to save the resulting Host Status
        #[clap(short, long)]
        status: Option<PathBuf>,

        /// Path to save an eventual fatal error
        #[clap(short, long)]
        error: Option<PathBuf>,
    },

    /// Rebuild software RAID arrays managed by Trident
    #[clap(name = "rebuild-raid")]
    RebuildRaid {
        /// The new configuration to work from
        #[clap(short, long)]
        config: Option<PathBuf>,

        /// Path to save the resulting HostStatus
        #[clap(short, long)]
        status: Option<PathBuf>,

        /// Path to save an eventual fatal error
        #[clap(short, long)]
        error: Option<PathBuf>,
    },

    /// Configure OS networking based on Trident Configuration
    #[clap(name = "start-network")]
    StartNetwork {
        /// The new configuration to apply
        #[clap(short, long, default_value = "/etc/trident/config.yaml")]
        config: PathBuf,
    },

    /// Get the HostStatus
    #[clap(name = "get")]
    GetHostStatus {
        /// Path to save the resulting HostStatus
        #[clap(short, long)]
        status: Option<PathBuf>,

        /// Output only the 'spec' field of the Host Status.
        #[clap(long, default_value = "false")]
        config_only: bool,
    },

    /// Validate HostConfiguration
    ///
    /// When no options are provided, the default Trident Configuration is
    /// validated.
    Validate {
        /// Path to a Host Configuration file
        #[clap(index = 1, default_value = "/etc/trident/config.yaml")]
        config: PathBuf,
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

impl Commands {
    pub fn name(&self) -> &'static str {
        match self {
            Commands::Run { .. } => "run",
            Commands::RebuildRaid { .. } => "rebuild-raid",
            Commands::StartNetwork { .. } => "start-network",
            Commands::GetHostStatus { .. } => "get-host-status",
            Commands::Validate { .. } => "validate",
            #[cfg(feature = "pytest-generator")]
            Commands::Pytest => "pytest",
            Commands::OfflineInitialize { .. } => "offline-initialize",
        }
    }
}

impl Display for Commands {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(f, "{}", self.name())
    }
}

struct AgentConfig {
    datastore: PathBuf,
}

fn load_agent_config() -> Result<AgentConfig, TridentError> {
    let mut config = AgentConfig {
        datastore: TRIDENT_DATASTORE_PATH_DEFAULT.into(),
    };

    if let Ok(contents) = std::fs::read_to_string(AGENT_CONFIG_PATH) {
        for line in contents.lines() {
            if let Some(path) = line.strip_prefix("DatastorePath=") {
                config.datastore = path.trim().into();
            }
        }
    }

    Ok(config)
}

fn run_trident(
    mut logstream: Logstream,
    mut tracestream: TraceStream,
    args: &Cli,
) -> Result<(), TridentError> {
    // Log version ASAP
    info!("Trident version: {}", trident::TRIDENT_VERSION);

    // Catch exit fast commands
    match &args.command {
        Commands::Validate { config } => {
            return validation::validate_host_config_file(config);
        }

        #[cfg(feature = "pytest-generator")]
        Commands::Pytest => {
            pytest::generate_functional_test_manifest();
            return Ok(());
        }

        Commands::OfflineInitialize { hs_path } => {
            return offline_init::execute(hs_path);
        }

        Commands::GetHostStatus {
            status,
            config_only,
        } => {
            return Trident::retrieve_host_status(
                &load_agent_config()?.datastore,
                status,
                *config_only,
            )
            .message("Failed to retrieve Host Status");
        }

        Commands::StartNetwork { config } => {
            // Lock the streams if we're starting the network
            // We have no network yet, so we can't send logs or traces anywhere
            logstream.disable();
            tracestream.disable();

            return Trident::start_network(HostConfigurationSource::File(config.clone()));
        }

        _ => (),
    }

    let res = panic::catch_unwind(move || {
        match &args.command {
            Commands::Run { status, error, .. } | Commands::RebuildRaid { status, error, .. } => {
                let mut config_path = match &args.command {
                    Commands::Run { config, .. } => Some(config.clone()),
                    Commands::RebuildRaid { config, .. } => config.clone(),
                    _ => None,
                };

                if let Some(path) = &config_path {
                    if !path.exists() {
                        warn!("Config file '{}' does not exist. Ignoring.", path.display());
                        config_path = None;
                    }
                }

                let agent_config = load_agent_config()?;

                let mut trident = Trident::new(
                    config_path.map(HostConfigurationSource::File),
                    &agent_config.datastore,
                    logstream,
                    tracestream,
                )
                .message("Failed to initialize Trident")?;

                // After initialization, create a trace event for the purpose of
                // measuring Trident reboot times
                tracing::info!(metric_name = "trident_start");

                // Execute the command
                let res = match args.command {
                    Commands::Run {
                        ref allowed_operations,
                        ..
                    } => {
                        let mut ops = Operations::empty();
                        if allowed_operations.contains(&AllowedOperation::Stage) {
                            ops.0.insert(Operation::Stage);
                        }
                        if allowed_operations.contains(&AllowedOperation::Finalize) {
                            ops.0.insert(Operation::Finalize);
                        }
                        trident.run(&agent_config.datastore, ops)
                    }
                    Commands::RebuildRaid { .. } => trident.rebuild_raid(&agent_config.datastore),
                    _ => Err(TridentError::internal("Invalid command")),
                };

                // return HostStatus if requested
                if status.is_some() {
                    if let Err(e) =
                        Trident::retrieve_host_status(&agent_config.datastore, status, false)
                            .message("Failed to retrieve Host Status")
                    {
                        error!("{e:?}");
                    }
                }

                // return error if requested
                if let Some(error_path) = error.as_ref() {
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

                res.message(format!("Failed to execute '{}' command", args.command))?;
            }
            _ => unreachable!(),
        }

        Ok(())
    });

    match res {
        Err(e) => Err(TridentError::new(InternalError::Panic(format!("{e:?}")))),
        Ok(r) => r,
    }
}

fn setup_logging(args: &Cli) -> Result<Logstream, Error> {
    let logstream = Logstream::create();

    // Set up the multilogger
    let mut multilogger = MultiLogger::new()
        // Add regular env_logger to output to stderr
        .with_logger(Box::new(
            env_logger::builder()
                .format_timestamp(None)
                .filter_level(args.verbosity)
                .build(),
        ))
        // Add logstream to send logs to the log server
        .with_logger(logstream.make_logger_with_level(LevelFilter::Trace))
        // Set the global filter for reqwest to debug
        .with_global_filter("reqwest", LevelFilter::Debug);

    // Add background logger if we're running a command that needs it
    if matches!(
        args.command,
        Commands::Run { .. } | Commands::RebuildRaid { .. }
    ) {
        multilogger.add_logger(BackgroundLog::new(TRIDENT_BACKGROUND_LOG_PATH).into_logger());
    }

    multilogger.init().context("Logger already registered")?;

    Ok(logstream)
}

fn setup_tracing(args: &Cli) -> Result<TraceStream, Error> {
    use tracing_subscriber::{filter, layer::SubscriberExt, Layer};

    let tracestream = TraceStream::default();

    if matches!(
        args.command,
        Commands::Run { .. } | Commands::RebuildRaid { .. }
    ) {
        // Set up the trace sender
        let trace_sender = tracestream
            .make_trace_sender()
            .with_filter(filter::LevelFilter::INFO);

        tracing::subscriber::set_global_default(
            tracing_subscriber::Registry::default().with(trace_sender),
        )
        .context("Failed to set global default subscriber")?;
    }

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
    let tracestream = setup_tracing(&args);
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
