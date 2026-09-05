use std::{fs, iter, panic, process::ExitCode, time::Duration};

use anyhow::{Context, Error};
use clap::Parser;
use log::{error, info, warn, LevelFilter, Log};

use osutils::logging::{filter::LogFilter, multilog::MultiLogger};
use trident::{
    agentconfig::AgentConfig,
    cli::{self, Cli, Commands, GetKind, TridentExitCodes},
    init::offline,
    manual_rollback::{self, utils::ManualRollbackRequestKind},
    run_with_operation, validation, AppInsightsSender, BackgroundLog, BackgroundUploader,
    DataStore, ExitKind, LogForwarder, Logstream, TraceStream, Trident,
    TRIDENT_BACKGROUND_LOG_PATH,
};
use trident_api::{
    config::{HostConfigurationSource, Operations},
    error::{InternalError, InvalidInputError, TridentError, TridentResultExt},
};

/// Maps a base command name plus its requested `Operations` to the same
/// naming convention gRPC's `servicing_request` already uses for
/// stage/finalize granularity (e.g. `"install"` vs `"install_stage"` vs
/// `"install_finalize"`), so `command`/`operation_id` telemetry is
/// consistent regardless of whether the command came from the CLI or from
/// gRPC/daemon.
fn command_name(base: &str, ops: &Operations) -> String {
    match (ops.has_stage(), ops.has_finalize()) {
        (true, true) | (false, false) => base.to_string(),
        (true, false) => format!("{base}_stage"),
        (false, true) => format!("{base}_finalize"),
    }
}

fn run_trident(
    mut logstream: Logstream,
    mut tracestream: TraceStream,
    args: &Cli,
) -> Result<ExitKind, TridentError> {
    // Log version ASAP
    info!("Trident version: {}", trident::TRIDENT_VERSION);

    // Log proxy environment for diagnostics (helps debug baremetal proxy issues)
    let proxy_status = |var: &str| -> &'static str {
        let lower = var.to_lowercase();
        if std::env::var(var)
            .or_else(|_| std::env::var(&lower))
            .ok()
            .filter(|v| !v.trim().is_empty())
            .is_some()
        {
            "<set>"
        } else {
            "<unset>"
        }
    };
    info!(
        "Proxy env: HTTPS_PROXY={}, HTTP_PROXY={}, NO_PROXY={}",
        proxy_status("HTTPS_PROXY"),
        proxy_status("HTTP_PROXY"),
        proxy_status("NO_PROXY"),
    );

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
            return offline::execute(
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

        // Handle diagnose command
        Commands::Diagnose {
            output,
            journal,
            selinux,
        } => {
            return Trident::diagnose(output, *journal, *selinux)
                .message("Failed to generate diagnostics")
                .map(|()| ExitKind::Done);
        }

        // Handle manual rollback check here so root is not required for --check
        Commands::Rollback {
            check: true,
            ab,
            runtime,
            ..
        } => {
            let datastore = DataStore::open_or_create(AgentConfig::load()?.datastore_path())
                .message("Failed to open datastore")?;
            return manual_rollback::check_rollback(
                &datastore,
                ManualRollbackRequestKind::from_flags(*runtime, *ab)?,
            )
            .message("Failed to check manual rollback availability")
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
            | Commands::RebuildRaid { status, error, .. }
            | Commands::Rollback { status, error, .. } => {
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
                // For non-install and non-update (update will check and has special handling for CIH
                // scenario) commands, we expect the datastore to exist
                if !matches!(
                    args.command,
                    Commands::Install { .. } | Commands::Update { .. }
                ) && !agent_config.datastore_path().exists()
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

                // `Trident::new` has already retrieved (or created) this
                // host's persisted correlation ID and attached it to the
                // shared TraceStream, so every trace/metric emitted from
                // here on -- including "trident_start" -- carries it.
                let mut datastore = DataStore::open_or_create(agent_config.datastore_path())
                    .message("Failed to open datastore")?;

                // Execute the command
                let res = match args.command {
                    Commands::Install {
                        ref allowed_operations,
                        multiboot,
                        ..
                    } => {
                        let ops = cli::to_operations(allowed_operations);
                        run_with_operation(&command_name("install", &ops), || {
                            trident
                                .install(&mut datastore, ops, multiboot, None)
                                .map(|(exit_kind, _image_hash, _servicing_type)| exit_kind)
                        })
                    }
                    Commands::Update {
                        ref allowed_operations,
                        ..
                    } => {
                        let ops = cli::to_operations(allowed_operations);
                        run_with_operation(&command_name("update", &ops), || {
                            trident
                                .update(&mut datastore, ops)
                                .map(|(exit_kind, _image_hash, _servicing_type)| exit_kind)
                        })
                    }
                    Commands::Commit { .. } => run_with_operation("commit", || {
                        trident
                            .commit(&mut datastore)
                            .map(|(exit_kind, _servicing_type)| exit_kind)
                    }),
                    Commands::Rollback {
                        runtime,
                        ab,
                        ref allowed_operations,
                        ..
                    } => {
                        let ops = cli::to_operations(allowed_operations);
                        run_with_operation(&command_name("rollback", &ops), || {
                            trident
                                .rollback(&mut datastore, runtime, ab, ops)
                                .map(|(exit_kind, _servicing_type)| exit_kind)
                        })
                    }
                    Commands::RebuildRaid { .. } => run_with_operation("rebuild_raid", || {
                        trident
                            .rebuild_raid(&mut datastore)
                            .map(|()| ExitKind::Done)
                    }),
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
    uploader: &BackgroundUploader,
    additional_loggers: impl Iterator<Item = Box<dyn Log>>,
) -> Result<Logstream, Error> {
    let logstream = Logstream::create(uploader.get_handle().context("Uploader is closed")?);

    // Set up the multilogger
    let mut multilogger = MultiLogger::new()
        // Add logstream to send logs to the log server
        .with_logger(logstream.make_logger_with_level(LevelFilter::Trace))
        // Set the global filter for reqwest to debug
        .with_global_filter("reqwest", LevelFilter::Debug)
        // Filter out debug logs from h2, some of which have target "tracing::span"
        .with_global_filter("tracing::span", LevelFilter::Error)
        .with_global_filter("h2", LevelFilter::Error)
        // Filter out this very noisy module that logs a lot when logstream is active.
        .with_global_filter("hyper_util::client", LevelFilter::Info);

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
            | Commands::Rollback { .. }
            | Commands::Daemon { .. }
    ) {
        multilogger.add_logger(BackgroundLog::new(TRIDENT_BACKGROUND_LOG_PATH).into_logger());
    }

    for logger in additional_loggers {
        multilogger.add_logger(logger);
    }

    multilogger.init().context("Logger already registered")?;

    Ok(logstream)
}

/// Whether the Application Insights tracing layer ended up active on this
/// invocation, and why not when it didn't. Computed by [`setup_tracing`] and
/// surfaced via [`TelemetryStatus::log`] once real logging is available, so
/// operators can tell -- from the logs alone, without reading source --
/// whether telemetry should be expected to actually reach Application
/// Insights, rather than silently assuming it based on the `Telemetry=`
/// setting alone (a bad/unreachable connection string, for example, fails
/// silently otherwise).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TelemetryStatus {
    /// Tracing/telemetry setup does not apply to this command at all (the
    /// `_ => {}` arm in [`setup_tracing`]) -- not logged.
    NotApplicable,
    /// `Telemetry=OptOut` (the default): telemetry was never attempted.
    OptedOut,
    /// Opted in, but no usable `AZURE_MONITOR_CONNECTION_STRING` was
    /// compiled into this binary at build time (missing, empty, or failed
    /// to parse).
    NoConnectionString,
    /// Opted in with a connection string, but the dedicated telemetry
    /// background uploader is unavailable (failed to start, or its handle
    /// was already closed).
    UploaderUnavailable,
    /// Opted in, connection string valid, uploader available: telemetry is
    /// actively being sent.
    Enabled,
}

impl TelemetryStatus {
    /// Log this status through the real logging pipeline. Must only be
    /// called after logging has been initialized (`setup_logging`) --
    /// calling it earlier would silently no-op, since the `log` facade
    /// drops everything until a logger is registered.
    fn log(self) {
        match self {
            TelemetryStatus::NotApplicable => {}
            TelemetryStatus::OptedOut => {
                info!(
                    "Telemetry: disabled (Telemetry=OptOut, the default, in agent configuration)"
                );
            }
            TelemetryStatus::NoConnectionString => {
                info!(
                    "Telemetry: opted in, but no usable Application Insights connection string \
                     was compiled into this binary -- telemetry is a no-op"
                );
            }
            TelemetryStatus::UploaderUnavailable => {
                warn!(
                    "Telemetry: opted in with a valid connection string, but the telemetry \
                     background uploader is unavailable -- telemetry is a no-op"
                );
            }
            TelemetryStatus::Enabled => {
                info!("Telemetry: enabled, sending tracing data to Application Insights");
            }
        }
    }
}

fn setup_tracing(
    args: &Cli,
    telemetry_enabled: bool,
    // Dedicated to Application Insights telemetry -- deliberately *not* the
    // same `BackgroundUploader` instance used for log forwarding (see
    // `main`), so a slow-but-successful telemetry endpoint can never build a
    // backlog that delays real log uploads. `None` if telemetry is disabled
    // or its uploader failed to start; either way telemetry becomes a no-op.
    telemetry_uploader: Option<&BackgroundUploader>,
) -> Result<(TraceStream, TelemetryStatus), Error> {
    use tracing_subscriber::{filter, layer::SubscriberExt, Layer, Registry};

    let tracestream = TraceStream::default();
    let mut telemetry_status = TelemetryStatus::NotApplicable;

    match &args.command {
        Commands::Commit { .. }
        | Commands::Daemon { .. }
        | Commands::GrpcClient { .. }
        | Commands::Install { .. }
        | Commands::RebuildRaid { .. }
        | Commands::Rollback { check: false, .. }
        | Commands::Update { .. } => {
            let mut layers: Vec<Box<dyn Layer<Registry> + Send + Sync>> = vec![Box::new(
                tracestream
                    .make_trace_sender()
                    .with_filter(filter::LevelFilter::INFO),
            )];

            // As functionality moves to the Daemon, move the journald layer to
            // only be enabled for the Daemon command. Until then, keep it enabled
            // for all commands to ensure we have tracing info in journald for all
            // commands.
            match tracing_journald::layer() {
                Ok(journald_layer) => {
                    layers.push(Box::new(
                        journald_layer
                            .with_syslog_identifier("trident-tracing".to_string())
                            .with_filter(filter::LevelFilter::INFO),
                    ));
                }
                Err(_) => {
                    eprintln!("Failed to connect to journald, falling back to tracing without journald support");
                }
            }

            // Best-effort Application Insights telemetry: only added when the
            // user has opted in via the Agent Configuration file *and* a
            // connection string was compiled into this binary at build time.
            // Never fails startup: an empty/unparsable connection string just
            // means telemetry stays a no-op. `telemetry_status` records which
            // of these applied so the caller can log it once real logging is
            // available (see `TelemetryStatus::log`).
            telemetry_status = if !telemetry_enabled {
                TelemetryStatus::OptedOut
            } else {
                // A missing/closed uploader (e.g. its background thread
                // failed to start) just means telemetry stays a no-op; it
                // must never block or fail the rest of tracing setup.
                match telemetry_uploader.and_then(|u| u.get_handle()) {
                    Some(handle) => match AppInsightsSender::from_connection_string(
                        trident::AZURE_MONITOR_CONNECTION_STRING,
                        handle,
                        tracestream.correlation_id_handle(),
                    ) {
                        Some(sender) => {
                            layers.push(Box::new(sender.with_filter(filter::LevelFilter::INFO)));
                            TelemetryStatus::Enabled
                        }
                        None => TelemetryStatus::NoConnectionString,
                    },
                    None => TelemetryStatus::UploaderUnavailable,
                }
            };

            tracing::subscriber::set_global_default(Registry::default().with(layers))
                .context("Failed to set global default subscriber")?;
        }
        _ => {
            // no op
        }
    }

    Ok((tracestream, telemetry_status))
}

/// How long to wait for the dedicated telemetry uploader to drain and
/// shut down before abandoning it (see
/// `BackgroundUploader::shutdown_with_deadline`). Telemetry must never
/// meaningfully delay Trident's actual work, including at shutdown -- a
/// slow-but-successful Application Insights endpoint could otherwise
/// stall process exit for as long as it takes to drain every queued
/// event.
const TELEMETRY_SHUTDOWN_DEADLINE: Duration = Duration::from_secs(5);

/// Wraps a `BackgroundUploader` so it is always shut down with a bounded
/// deadline when dropped, regardless of which of `main`'s many return
/// points is taken -- `BackgroundUploader`'s own `Drop` impl (used
/// elsewhere, e.g. for `bg_uploader`, which carries real log delivery and
/// is expected to drain fully) waits unboundedly instead.
struct TelemetryUploaderGuard(Option<BackgroundUploader>);

impl Drop for TelemetryUploaderGuard {
    fn drop(&mut self) {
        if let Some(uploader) = self.0.take() {
            uploader.shutdown_with_deadline(TELEMETRY_SHUTDOWN_DEADLINE);
        }
    }
}

fn main() -> ExitCode {
    // Parse args
    let args = Cli::parse();

    let bg_uploader = match BackgroundUploader::new() {
        Ok(uploader) => uploader,
        Err(e) => {
            // Defer to stderr since logging is not yet initialized.
            eprintln!("Failed to initialize background uploader: {e:?}");
            return TridentExitCodes::SetupFailed.into();
        }
    };

    // Whether best-effort Application Insights telemetry is enabled. Loaded
    // early (before logging/tracing is set up) since the decision feeds
    // directly into setup_tracing(). AgentConfig::load() never actually
    // errors today, but default to disabled (OptOut) defensively if that
    // ever changes.
    let telemetry_enabled = AgentConfig::load()
        .map(|config| config.telemetry_enabled())
        .unwrap_or(false);

    // Application Insights telemetry gets its own dedicated uploader/queue,
    // entirely separate from `bg_uploader` (which carries real log
    // forwarding). Both uploaders drain their queue sequentially on a single
    // background thread, so sharing one between telemetry and logs would let
    // a slow-but-successful telemetry endpoint build a backlog that delays
    // operational log uploads. Failure to start is not fatal: telemetry
    // simply becomes a no-op, mirroring failure handling on the handle
    // itself.
    let telemetry_uploader = telemetry_enabled
        .then(|| match BackgroundUploader::new() {
            Ok(uploader) => Some(uploader),
            Err(e) => {
                eprintln!("Failed to initialize telemetry uploader, disabling telemetry: {e:?}");
                None
            }
        })
        .flatten();
    // Wrapped immediately so every return path in main() below shuts it
    // down with a bounded deadline, not BackgroundUploader's own unbounded
    // Drop.
    let telemetry_uploader = TelemetryUploaderGuard(telemetry_uploader);

    // Initialize the telemetry flow
    let tracing_setup = setup_tracing(&args, telemetry_enabled, telemetry_uploader.0.as_ref());
    if let Err(e) = tracing_setup {
        // Defer to stderr since logging is not yet initialized.
        eprintln!("Failed to initialize tracing: {e:?}");
        return TridentExitCodes::SetupFailed.into();
    }
    let (tracestream, telemetry_status) = tracing_setup.unwrap();

    if let Commands::Daemon {
        inactivity_timeout,
        socket_path,
    } = &args.command
    {
        let log_forwarder = LogForwarder::default();
        // Initialize the loggers
        let logstream = setup_logging(
            &args,
            &bg_uploader,
            [LogFilter::new(log_forwarder.new_logger())
                .with_global_filter("trident::server", LevelFilter::Off)
                .with_global_filter("tonic", LevelFilter::Error)
                .with_global_filter("h2", LevelFilter::Error)
                .into_logger() as Box<dyn Log>]
            .into_iter(),
        );
        if let Err(e) = logstream {
            error!("Failed to initialize logging: {e:?}");
            return TridentExitCodes::SetupFailed.into();
        }

        // Log version on startup
        info!("Trident version: {}", trident::TRIDENT_VERSION);
        telemetry_status.log();

        trident::server_main(
            log_forwarder,
            *inactivity_timeout,
            socket_path,
            logstream.unwrap(),
            tracestream,
        )
    } else if let Commands::GrpcClient(client_args) = &args.command {
        let logstream = setup_logging(&args, &bg_uploader, iter::empty());
        if let Err(e) = logstream {
            error!("Failed to initialize logging: {e:?}");
            return TridentExitCodes::SetupFailed.into();
        }

        if let Err(e) = logstream.unwrap().try_initialize_from_env() {
            error!("Failed to initialize logstream from environment: {e:?}");
        }

        telemetry_status.log();

        // Run the client command
        trident::client_main(client_args)
    } else {
        // Initialize the loggers
        let logstream = setup_logging(&args, &bg_uploader, iter::empty());
        if let Err(e) = logstream {
            error!("Failed to initialize logging: {e:?}");
            return TridentExitCodes::SetupFailed.into();
        }

        telemetry_status.log();

        // Invoke Trident
        match run_trident(logstream.unwrap(), tracestream, &args) {
            Ok(ExitKind::Done) => {}
            Err(e) => {
                error!("{e:?}");
                return TridentExitCodes::Failed.into();
            }
            Ok(ExitKind::NeedsReboot) => {
                if let Err(e) = trident::request_reboot_with_wait() {
                    error!("Failed to reboot: {e:?}");
                    return TridentExitCodes::RebootUnsuccessful.into();
                }
            }
        }

        TridentExitCodes::Success.into()
    }
}
