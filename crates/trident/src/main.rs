use std::{fs, iter, panic, process::ExitCode};

use anyhow::{Context, Error};
use clap::Parser;
use log::{error, info, LevelFilter, Log};

use trident::{
    agentconfig::AgentConfig,
    cli::{self, Cli, Commands, GetKind},
    offline_init, validation, BackgroundLog, DataStore, ExitKind, LogFilter, LogForwarder,
    Logstream, MultiLogger, TraceStream, Trident, TRIDENT_BACKGROUND_LOG_PATH,
};
use trident_api::{
    config::HostConfigurationSource,
    error::{InternalError, InvalidInputError, TridentError, TridentResultExt},
};

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
            history_path,
        } => {
            return offline_init::execute(
                hs_path.as_deref(),
                lazy_partitions,
                disk,
                history_path.as_deref(),
            )
            .map(|()| ExitKind::Done);
        }

        Commands::Get { kind, outfile } => {
            return Trident::get(AgentConfig::load()?.datastore_path(), outfile, *kind)
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

        #[cfg(feature = "dangerous-options")]
        Commands::StreamImage {
            image,
            hash,
            status,
            error,
            ..
        } => {
            use std::io::Write;
            use trident_api::error::ReportError;

            let config = trident::stream::config_from_image_url(image.clone(), hash)
                .message("Failed to generate Host Configuration from image URL")?;

            // Write config to a temporary file
            let file = tempfile::NamedTempFile::new()
                .structured(InternalError::Internal("serialize host config"))?;
            file.as_file()
                .write_all(
                    serde_yaml::to_string(&config)
                        .structured(InternalError::Internal("serialize host config"))?
                        .as_bytes(),
                )
                .structured(InternalError::Internal("serialize host config"))?;

            return run_trident(
                logstream,
                tracestream,
                &Cli {
                    command: Commands::Install {
                        config: file.path().to_path_buf(),
                        allowed_operations: vec![
                            trident::cli::AllowedOperation::Stage,
                            trident::cli::AllowedOperation::Finalize,
                        ],
                        status: status.clone(),
                        error: error.clone(),
                        multiboot: false,
                    },
                    verbosity: args.verbosity,
                },
            );
        }

        _ => (),
    }

    let res = panic::catch_unwind(move || {
        match &args.command {
            Commands::Install { status, error, .. }
            | Commands::Update { status, error, .. }
            | Commands::Commit { status, error }
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

                let agent_config = AgentConfig::load()?;
                // For non-install commands, we expect the datastore to exist
                if !matches!(args.command, Commands::Install { .. })
                    && !agent_config.datastore_path().exists()
                {
                    return Err(TridentError::new(InvalidInputError::HostNotProvisioned))
                        .message("Datastore file does not exist");
                }

                let mut trident = Trident::new(
                    config_path.map(HostConfigurationSource::File),
                    agent_config.datastore_path(),
                    logstream,
                    tracestream,
                )
                .message("Failed to initialize Trident")?;

                // After initialization, create a trace event for the purpose of
                // measuring Trident reboot times
                tracing::info!(metric_name = "trident_start");

                let mut datastore = DataStore::open_or_create(agent_config.datastore_path())
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
                    ),
                    Commands::Update {
                        ref allowed_operations,
                        ..
                    } => trident.update(&mut datastore, cli::to_operations(allowed_operations)),
                    Commands::Commit { .. } => trident.commit(&mut datastore),
                    Commands::RebuildRaid { .. } => trident
                        .rebuild_raid(&mut datastore)
                        .map(|()| ExitKind::Done),
                    _ => Err(TridentError::internal("Invalid command")),
                };

                // Return Host Status if requested
                if status.is_some() {
                    if let Err(e) =
                        Trident::get(agent_config.datastore_path(), status, GetKind::Status)
                            .message("Failed to retrieve Host Status")
                    {
                        error!("{e:?}");
                    }
                }

                // Return error if requested
                if let Some(error_path) = error.as_ref() {
                    if let Err(e) = &res {
                        if let Err(e2) =
                            fs::write(error_path, serde_yaml::to_string(&e).unwrap_or("".into()))
                        {
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

fn setup_logging(
    args: &Cli,
    additional_loggers: impl Iterator<Item = Box<dyn Log>>,
) -> Result<Logstream, Error> {
    let logstream = Logstream::create();

    // Set up the multilogger
    let mut multilogger = MultiLogger::new()
        // Add logstream to send logs to the log server
        .with_logger(logstream.make_logger_with_level(LevelFilter::Trace))
        // Set the global filter for reqwest to debug
        .with_global_filter("reqwest", LevelFilter::Debug)
        // Filter out debug logs from h2, some of which have target "tracing::span"
        .with_global_filter("tracing::span", LevelFilter::Error)
        .with_global_filter("h2", LevelFilter::Error);

    // Attempt to use the systemd journal if stderr is directly connected to it, and otherwise fall
    // back to env_logger.
    if let Some(Ok(journal_logger)) =
        systemd_journal_logger::connected_to_journal().then(systemd_journal_logger::JournalLog::new)
    {
        multilogger.add_logger(Box::new(
            journal_logger.with_extra_fields(vec![("VERSION", trident::TRIDENT_VERSION)]),
        ));
    } else {
        multilogger.add_logger(Box::new(
            env_logger::builder()
                .format_timestamp(None)
                .filter_level(args.verbosity)
                .build(),
        ));
    }

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

    for logger in additional_loggers {
        multilogger.add_logger(logger);
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

    // Initialize the telemetry flow
    let tracestream = setup_tracing(&args);
    if let Err(e) = tracestream {
        error!("Failed to initialize tracing: {e:?}");
        return ExitCode::from(1);
    }

    if let Commands::Daemon {
        inactivity_timeout,
        socket_path,
    } = &args.command
    {
        let log_forwarder = LogForwarder::default();
        // Initialize the loggers
        let logstream = setup_logging(
            &args,
            [LogFilter::new(log_forwarder.new_logger())
                .with_global_filter("trident::server", LevelFilter::Off)
                .with_global_filter("tonic", LevelFilter::Error)
                .with_global_filter("h2", LevelFilter::Error)
                .into_logger() as Box<dyn Log>]
            .into_iter(),
        );
        info!("Trident version: {}", trident::TRIDENT_VERSION);
        if let Err(e) = logstream {
            error!("Failed to initialize logging: {e:?}");
            return ExitCode::from(1);
        }

        trident::server_main(
            log_forwarder,
            *inactivity_timeout,
            socket_path,
            logstream.unwrap(),
            tracestream.unwrap(),
        )
    } else {
        // Initialize the loggers
        let logstream = setup_logging(&args, iter::empty());
        if let Err(e) = logstream {
            error!("Failed to initialize logging: {e:?}");
            return ExitCode::from(1);
        }

        // Invoke Trident
        match run_trident(logstream.unwrap(), tracestream.unwrap(), &args) {
            Ok(ExitKind::Done) => {}
            Err(e) => {
                error!("{e:?}");
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
}
