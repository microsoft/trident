use std::{
    fmt::{Display, Formatter, Result as FmtResult},
    panic,
    path::PathBuf,
    process::ExitCode,
};

use anyhow::{Context, Error};
use clap::{Parser, Subcommand};
use log::{error, info, LevelFilter};

use trident::{
    offline_init, validation, BackgroundLog, DataStore, GetKind, Logstream, MultiLogger,
    TraceStream, Trident, TRIDENT_BACKGROUND_LOG_PATH,
};
use trident_api::{
    config::{HostConfigurationSource, Operation, Operations},
    constants::{AGENT_CONFIG_PATH, TRIDENT_DATASTORE_PATH_DEFAULT},
    error::{InternalError, InvalidInputError, TridentError, TridentResultExt},
};

#[derive(Parser, Debug)]
#[clap(version = trident::TRIDENT_VERSION)]
struct Cli {
    /// Logging verbosity [OFF, ERROR, WARN, INFO, DEBUG, TRACE]
    #[arg(global = true, short, long, default_value_t = LevelFilter::Debug)]
    verbosity: LevelFilter,

    #[clap(subcommand)]
    command: Commands,
}

#[derive(clap::ValueEnum, Clone, Debug, Eq, PartialEq)]
enum AllowedOperation {
    Stage,
    Finalize,
}
fn to_operations(allowed_operations: &[AllowedOperation]) -> Operations {
    let mut ops = Operations::empty();
    for op in allowed_operations {
        match op {
            AllowedOperation::Stage => ops.0.insert(Operation::Stage),
            AllowedOperation::Finalize => ops.0.insert(Operation::Finalize),
        };
    }
    ops
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Initiate an install of Azure Linux
    Install {
        /// The new configuration to apply
        #[clap(index = 1, default_value = "/etc/trident/config.yaml")]
        config: PathBuf,

        #[clap(long, value_delimiter = ',', num_args = 0.., default_value = "stage,finalize")]
        allowed_operations: Vec<AllowedOperation>,

        /// Path to save the resulting Host Status
        #[clap(short, long)]
        status: Option<PathBuf>,

        /// Path to save an eventual fatal error
        #[clap(short, long)]
        error: Option<PathBuf>,

        /// Allow Trident to perform a multiboot install
        #[clap(long)]
        multiboot: bool,
    },

    /// Start or continue an A/B update from an existing install
    Update {
        /// The new configuration to apply
        #[clap(index = 1, default_value = "/etc/trident/config.yaml")]
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

    /// Detect whether an install or update succeeded, and update the boot order accordingly
    Commit {
        /// Path to save the resulting Host Status
        #[clap(short, long)]
        status: Option<PathBuf>,

        /// Path to save an eventual fatal error
        #[clap(short, long)]
        error: Option<PathBuf>,
    },

    #[clap(hide(true))]
    Listen {
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
    #[clap(name = "start-network", hide(true))]
    StartNetwork {
        /// The new configuration to apply
        #[clap(index = 1, default_value = "/etc/trident/config.yaml")]
        config: PathBuf,
    },

    /// Query the current state of the system
    #[clap(name = "get")]
    Get {
        /// What data to retrieve
        #[clap(default_value = "status")]
        kind: GetKind,

        /// Path to save the resulting output
        #[clap(short, long)]
        outfile: Option<PathBuf>,
    },

    /// Validate the provided Host Configuration
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

    /// Initialize for a system that wasn't installed by Trident
    OfflineInitialize {
        /// Path to a Host Status file (deprecated)
        ///
        /// If not provided, Trident will infer one based on the state of the system and history
        /// information left by Image Customizer.
        hs_path: Option<PathBuf>,
    },
}

impl Commands {
    pub fn name(&self) -> &'static str {
        match self {
            Commands::Install { .. } => "install",
            Commands::Update { .. } => "update",
            Commands::Commit { .. } => "commit",
            Commands::Listen { .. } => "listen",
            Commands::RebuildRaid { .. } => "rebuild-raid",
            Commands::StartNetwork { .. } => "start-network",
            Commands::Get { .. } => "get",
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
            return offline_init::execute(hs_path.as_deref());
        }

        Commands::Get { kind, outfile } => {
            return Trident::get(&load_agent_config()?.datastore, outfile, *kind)
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
            Commands::Install { status, error, .. }
            | Commands::Update { status, error, .. }
            | Commands::Commit { status, error }
            | Commands::Listen { status, error }
            | Commands::RebuildRaid { status, error, .. } => {
                let config_path = match &args.command {
                    Commands::Update { config, .. } | Commands::Install { config, .. } => {
                        Some(config.clone())
                    }
                    Commands::RebuildRaid { config, .. } => config.clone(),
                    _ => None,
                };

                if let Some(path) = &config_path {
                    if !path.exists() {
                        return Err(TridentError::new(InvalidInputError::ReadInputFile {
                            path: path.to_string_lossy().to_string(),
                        }))
                        .message("Config file does not exist");
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

                let mut datastore = DataStore::open_or_create(&agent_config.datastore)
                    .message("Failed to open datastore")?;

                // Execute the command
                let res = match args.command {
                    Commands::Install {
                        ref allowed_operations,
                        multiboot,
                        ..
                    } => trident.install(
                        &mut datastore,
                        to_operations(allowed_operations),
                        multiboot,
                        #[cfg(feature = "grpc-dangerous")]
                        &mut None,
                    ),
                    Commands::Update {
                        ref allowed_operations,
                        ..
                    } => trident.update(
                        &mut datastore,
                        to_operations(allowed_operations),
                        #[cfg(feature = "grpc-dangerous")]
                        &mut None,
                    ),
                    Commands::Commit { .. } => trident.commit(&mut datastore),
                    Commands::Listen { .. } => trident.listen(&mut datastore),
                    Commands::RebuildRaid { .. } => trident.rebuild_raid(&mut datastore),
                    _ => Err(TridentError::internal("Invalid command")),
                };

                // return HostStatus if requested
                if status.is_some() {
                    if let Err(e) = Trident::get(&agent_config.datastore, status, GetKind::Status)
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
        Commands::Install { .. }
            | Commands::Update { .. }
            | Commands::Commit { .. }
            | Commands::RebuildRaid { .. }
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
        Commands::Install { .. }
            | Commands::Update { .. }
            | Commands::Commit { .. }
            | Commands::RebuildRaid { .. }
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
