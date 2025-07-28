use std::{panic, path::PathBuf, process::ExitCode};

use anyhow::{Context, Error};
use clap::Parser;
use log::{error, info, LevelFilter};

use trident::{
    cli::{self, Cli, Commands, GetKind},
    offline_init, validation, BackgroundLog, DataStore, ExitKind, Logstream, MultiLogger,
    TraceStream, Trident, TRIDENT_BACKGROUND_LOG_PATH,
};
use trident_api::{
    config::HostConfigurationSource,
    constants::{AGENT_CONFIG_PATH, TRIDENT_DATASTORE_PATH_DEFAULT},
    error::{InternalError, InvalidInputError, TridentError, TridentResultExt},
};

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
) -> Result<ExitKind, TridentError> {
    // Log version ASAP
    info!("Trident version: {}", trident::TRIDENT_VERSION);

    // Catch exit fast commands
    match &args.command {
        Commands::Validate { config } => {
            return validation::validate_host_config_file(config).map(|()| ExitKind::Done);
        }

        #[cfg(feature = "pytest-generator")]
        Commands::Pytest => {
            pytest::generate_functional_test_manifest();
            return Ok(ExitKind::Done);
        }

        Commands::OfflineInitialize {
            hs_path,
            lazy_partitions,
            disk,
        } => {
            return offline_init::execute(hs_path.as_deref(), lazy_partitions, disk)
                .map(|()| ExitKind::Done);
        }

        Commands::Get { kind, outfile } => {
            return Trident::get(&load_agent_config()?.datastore, outfile, *kind)
                .message("Failed to retrieve Host Status")
                .map(|()| ExitKind::Done);
        }

        Commands::StartNetwork { config } => {
            // Lock the streams if we're starting the network
            // We have no network yet, so we can't send logs or traces anywhere
            logstream.disable();
            tracestream.disable();

            return Trident::start_network(HostConfigurationSource::File(config.clone()))
                .map(|()| ExitKind::Done);
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
                // For non-install commands, we expect the datastore to exist
                if !matches!(args.command, Commands::Install { .. })
                    && !agent_config.datastore.exists()
                {
                    return Err(TridentError::new(InvalidInputError::HostNotProvisioned))
                        .message("Datastore file does not exist");
                }

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
                        cli::to_operations(allowed_operations),
                        multiboot,
                        #[cfg(feature = "grpc-dangerous")]
                        &mut None,
                    ),
                    Commands::Update {
                        ref allowed_operations,
                        ..
                    } => trident.update(
                        &mut datastore,
                        cli::to_operations(allowed_operations),
                        #[cfg(feature = "grpc-dangerous")]
                        &mut None,
                    ),
                    Commands::Commit { .. } => {
                        trident.commit(&mut datastore).map(|()| ExitKind::Done)
                    }
                    Commands::Listen { .. } => {
                        trident.listen(&mut datastore).map(|()| ExitKind::Done)
                    }
                    Commands::RebuildRaid { .. } => trident
                        .rebuild_raid(&mut datastore)
                        .map(|()| ExitKind::Done),
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

                res.message(format!("Failed to execute '{}' command", args.command))
            }
            _ => unreachable!(),
        }
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
        .with_global_filter("reqwest", LevelFilter::Debug)
        // Set the global filter for goblin to off
        .with_global_filter("goblin", LevelFilter::Off);

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
    match run_trident(logstream.unwrap(), tracestream.unwrap(), &args) {
        Ok(ExitKind::Done) => {}
        Err(e) => {
            error!("Trident failed: {e:?}");
            return ExitCode::from(2);
        }
        Ok(ExitKind::NeedsReboot) => {
            if let Err(e) = trident::reboot() {
                error!("Failed to reboot: {e:?}");
                return ExitCode::from(3);
            }
        }
    }
    ExitCode::SUCCESS
}
