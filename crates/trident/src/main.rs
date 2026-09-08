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
    run_command, run_reboot_command, save_reboot_operation, validation, AppInsightsSender,
    BackgroundLog, BackgroundUploader, DataStore, ExitKind, LogForwarder, Logstream,
    OperationSource, TraceStream, Trident, TRIDENT_BACKGROUND_LOG_PATH,
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
        (true, true) => base.to_string(),
        (true, false) => format!("{base}_stage"),
        (false, true) => format!("{base}_finalize"),
        // Neither stage nor finalize was requested (an empty
        // `--allowed-operations` list); nothing can actually be staged or
        // finalized, so name this like the existing no-op naming convention
        // rather than a full install/update.
        (false, false) => format!("{base}_noop"),
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

    // Fast-exit commands: read-only/one-shot commands that never start a
    // servicing run (validate, get, diagnose, offline-initialize, a manual
    // rollback --check, start-network). These deliberately run outside
    // run_command below -- no command_start/command_error telemetry is
    // emitted for them. Their failures (a malformed --config, a datastore
    // that can't be opened, a diagnostics bundle that can't be written,
    // etc.) are operator-input or read errors, not servicing outcomes; a
    // genuine underlying datastore/host problem still gets telemetry when
    // the actual servicing operation (install/update/etc.) that triggered
    // it runs. Handled here, before `command`/run_command are even set up,
    // so none of that machinery needs to reason about them.
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

    // Only servicing commands reach here: Install, Update, Commit,
    // RebuildRaid, and a non-check Rollback. These get command_start/
    // command_error telemetry via run_command below; the fast-exit
    // commands above already returned without any.
    let command = match &args.command {
        Commands::Install {
            allowed_operations, ..
        } => command_name(args.command.name(), &cli::to_operations(allowed_operations)),
        Commands::Update {
            allowed_operations, ..
        } => command_name(args.command.name(), &cli::to_operations(allowed_operations)),
        Commands::Rollback {
            allowed_operations, ..
        } => command_name(args.command.name(), &cli::to_operations(allowed_operations)),
        Commands::Commit { .. } | Commands::RebuildRaid { .. } => {
            args.command.name().replace('-', "_")
        }
        Commands::StartNetwork { .. }
        | Commands::Get { .. }
        | Commands::Diagnose { .. }
        | Commands::Validate { .. }
        | Commands::OfflineInitialize { .. } => {
            unreachable!("fast-exit commands already returned above")
        }
        #[cfg(feature = "pytest-generator")]
        Commands::Pytest => unreachable!("fast-exit commands already returned above"),
        Commands::Daemon { .. } | Commands::GrpcClient(_) => {
            unreachable!("Daemon/GrpcClient are dispatched in main(), never reach run_trident")
        }
    };

    // Attach this host's installation ID and database ID to the shared
    // TraceStream before run_command below fires command_start:
    // Trident::new (further down, inside the closure) is the usual place
    // both get attached, but that's too late for command_start, which
    // run_command fires immediately, before the closure even runs. Both
    // are read-only and side-effect-free: neither creates a datastore or
    // an ID (see `TraceStream::attach_installation_id_if_present` and
    // `TraceStream::attach_database_id_if_present`) -- silently does
    // nothing if the datastore doesn't exist yet, which is expected for a
    // host's first-ever install.
    if let Ok(agent_config) = AgentConfig::load() {
        tracestream.attach_installation_id_if_present(agent_config.datastore_path());
        tracestream.attach_database_id_if_present(agent_config.datastore_path());
    }

    // Determined up front so a missing/nonexistent --config is rejected
    // immediately, before run_command below even fires command_start.
    let config_path = match &args.command {
        Commands::Update { config, .. } | Commands::Install { config, .. } => Some(config.clone()),
        Commands::RebuildRaid { config, .. } => config.clone(),
        _ => None,
    };
    if let Some(path) = &config_path {
        if !path.exists() {
            return run_command(&command, OperationSource::Cli, || {
                Err(TridentError::new(InvalidInputError::ReadInputFile {
                    path: path.to_string_lossy().to_string(),
                }))
                .message("Config file does not exist")
            });
        }
    }

    // run_command itself now catches a panic from its closure (while the
    // operation context is still active) and fires command_error before
    // re-raising it, so a genuine panic gets the same telemetry as a
    // normal Err. This outer catch_unwind remains as a safety net for a
    // panic occurring outside run_command's closure (e.g. in run_command's
    // own setup) and to keep converting an unwound panic into a non-zero
    // exit code below.
    let res = panic::catch_unwind(move || {
        run_command(&command, OperationSource::Cli, || {
            match &args.command {
                Commands::Install { status, error, .. }
                | Commands::Update { status, error, .. }
                | Commands::Commit { status, error }
                | Commands::RebuildRaid { status, error, .. }
                | Commands::Rollback { status, error, .. } => {
                    // config_path was already validated (existence-checked)
                    // above.
                    let config_path = match &args.command {
                        Commands::Update { config, .. } | Commands::Install { config, .. } => {
                            Some(config.clone())
                        }
                        Commands::RebuildRaid { config, .. } => config.clone(),
                        _ => None,
                    };

                    let agent_config = AgentConfig::load()?;
                    // For commands that cannot themselves stage a new
                    // install/update (see
                    // `DataStore::may_initialize_datastore_for_command`),
                    // we expect the datastore to already exist. Update has
                    // its own special handling for the CIH bootstrap
                    // scenario further down.
                    if !DataStore::may_initialize_datastore_for_command(&command)
                        && !agent_config.datastore_path().exists()
                    {
                        return Err(TridentError::new(InvalidInputError::HostNotProvisioned))
                            .message("Datastore file does not exist");
                    }

                    // A multiboot install may swap to a brand-new temporary
                    // datastore inside `Trident::install` (see there),
                    // distinct from `datastore_path` here (the existing
                    // host's persistent datastore) -- so defer attaching an
                    // installation ID until `install` has settled on which
                    // datastore it actually uses, rather than attaching the
                    // existing host's here and having it be wrong for the
                    // rest of the run. See `new_deferring_installation_id`'s
                    // doc comment for the full rationale.
                    let defer_installation_id = matches!(
                        args.command,
                        Commands::Install {
                            multiboot: true,
                            ..
                        }
                    );
                    let mut trident = if defer_installation_id {
                        Trident::new_deferring_installation_id(
                            config_path.map(HostConfigurationSource::File),
                            agent_config.datastore_path(),
                            logstream.clone(),
                            tracestream.clone(),
                        )
                    } else {
                        Trident::new(
                            config_path.map(HostConfigurationSource::File),
                            agent_config.datastore_path(),
                            logstream.clone(),
                            tracestream.clone(),
                        )
                    }
                    .message("Failed to initialize Trident")?;

                    // `Trident::new` (or `Trident::new_deferring_installation_id`
                    // for a multiboot install) has already attached this
                    // host's persisted installation ID -- if any -- to the
                    // shared TraceStream, so every trace/metric emitted
                    // from here on -- including "trident_start" -- carries
                    // it once available.
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
                            trident
                                .install(&mut datastore, ops, multiboot, None)
                                .map(|(exit_kind, _image_hash, _servicing_type)| exit_kind)
                        }
                        Commands::Update {
                            ref allowed_operations,
                            ..
                        } => {
                            let ops = cli::to_operations(allowed_operations);
                            trident
                                .update(&mut datastore, ops)
                                .map(|(exit_kind, _image_hash, _servicing_type)| exit_kind)
                        }
                        Commands::Commit { .. } => trident
                            .commit(&mut datastore)
                            .map(|(exit_kind, _servicing_type)| exit_kind),
                        Commands::Rollback {
                            runtime,
                            ab,
                            ref allowed_operations,
                            ..
                        } => {
                            let ops = cli::to_operations(allowed_operations);
                            trident
                                .rollback(&mut datastore, runtime, ab, ops)
                                .map(|(exit_kind, _servicing_type)| exit_kind)
                        }
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
                            if let Err(e2) = fs::write(
                                error_path,
                                serde_yaml::to_string(&e).unwrap_or("".into()),
                            ) {
                                error!("Failed to write error to file: {e2}");
                            }
                        }
                    }

                    // Capture this operation's identity while its context
                    // is still installed (this closure runs entirely
                    // inside `run_command`'s scope), so the reboot
                    // requested below by the caller can be tagged with the
                    // *original* install/update/etc.'s `operation_id`/
                    // `command` instead of a disconnected fresh one -- see
                    // `save_reboot_operation`/`take_reboot_operation`.
                    if matches!(res, Ok(ExitKind::NeedsReboot)) {
                        save_reboot_operation();
                    }

                    res.message(format!("Failed to execute '{}' command", args.command))
                }
                _ => unreachable!(),
            }
        })
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
    /// `Commands::Pytest` arm in [`setup_tracing`], gated behind the
    /// `pytest-generator` feature) -- not logged. Only ever constructed
    /// when that feature is enabled; every other command now gets a real
    /// subscriber installed.
    #[cfg_attr(not(feature = "pytest-generator"), allow(dead_code))]
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
                    "Telemetry: opted in, but the telemetry background uploader is \
                     unavailable -- telemetry is a no-op"
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
    let telemetry_status;

    // Every command reachable from run_trident needs a subscriber
    // installed here -- not just the servicing ones -- so that ordinary
    // logging (journald) and, for the servicing commands, command_start/
    // command_error all reach a real subscriber instead of the default
    // one, which is none at all: tracing silently drops every event with
    // no subscriber installed. The fast "exit early" commands (validate,
    // get, diagnose, offline-initialize, rollback --check, start-network)
    // don't emit command_start/command_error or any other metric_name
    // event themselves (see run_trident), but still get a subscriber here
    // -- see the local_sender truncate-vs-append comment below for why.
    // StartNetwork's own `tracestream.disable()` (see run_trident) still
    // applies regardless -- it only suppresses a later `set_server` call
    // from configuring a remote phone-home target before the network
    // exists, not the local metrics-file/journald layers installed here,
    // which need no network.
    match &args.command {
        Commands::Commit { .. }
        | Commands::Daemon { .. }
        | Commands::GrpcClient { .. }
        | Commands::Install { .. }
        | Commands::RebuildRaid { .. }
        | Commands::Rollback { .. }
        | Commands::Update { .. }
        | Commands::Validate { .. }
        | Commands::Get { .. }
        | Commands::Diagnose { .. }
        | Commands::OfflineInitialize { .. }
        | Commands::StartNetwork { .. } => {
            // Truncating the local metrics file is only appropriate for a
            // process that actually owns the file's lifecycle for a fresh
            // servicing run -- Install/Update/Commit/RebuildRaid/Rollback
            // (finalize)/Daemon -- since those are the operations whose
            // metrics history is meaningful to reset per invocation. Every
            // other command reads or inspects existing state without
            // mutating it, so it must append instead of truncating -- not
            // because any of them emit command_start/command_error or any
            // other metric_name event of their own (they don't; see
            // run_trident), but because simply *opening* the file with
            // truncation is itself destructive:
            // * `validate`, `get`, `diagnose`, `offline-initialize`,
            //   `start-network`, and a manual rollback `--check` are all
            //   read-only/fast commands that never start a servicing run --
            //   truncating here would erase the preceding servicing
            //   metrics history just because one of these ran afterward.
            // * `Commands::GrpcClient` -- *every* subcommand of it, not
            //   just its own read-only ones (`get`, `validate`,
            //   `rollback --check`) -- never owns this file either way: by
            //   definition it only ever talks to an *already-running*
            //   daemon, which is the file's sole owner for as long as it's
            //   up. That makes truncation from a grpc-client process
            //   incorrect unconditionally, including for
            //   install/update/rollback (finalize) subcommands: those
            //   still start a *servicing* run, but that run is owned and
            //   recorded by the daemon, not by the short-lived grpc-client
            //   process asking for it. Previously only the read-only
            //   grpc-client subcommands were special-cased here, so a
            //   `grpc-client install`/`update`/`rollback` truncated the
            //   shared local metrics file out from under the daemon's own
            //   concurrent appends -- the exact race this append-mode
            //   split was meant to prevent.
            let local_sender = if matches!(
                args.command,
                Commands::GrpcClient(_)
                    | Commands::Diagnose { .. }
                    | Commands::Validate { .. }
                    | Commands::Get { .. }
                    | Commands::OfflineInitialize { .. }
                    | Commands::StartNetwork { .. }
                    | Commands::Rollback { check: true, .. }
            ) {
                tracestream.make_trace_sender_appending()
            } else {
                tracestream.make_trace_sender()
            };
            let mut layers: Vec<Box<dyn Layer<Registry> + Send + Sync>> = vec![Box::new(
                local_sender.with_filter(filter::LevelFilter::INFO),
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
                        tracestream.installation_id_handle(),
                        tracestream.database_id_handle(),
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
        // pytest-generator does no meaningful work of its own (just
        // generates functional-test wrappers at build/dev time) -- no
        // telemetry needed. Listed explicitly, rather than via a wildcard
        // fallback, so the compiler forces this match to be revisited
        // whenever a new command variant is added, instead of it silently
        // falling through to "no subscriber" the way the commands above
        // used to.
        #[cfg(feature = "pytest-generator")]
        Commands::Pytest => {
            telemetry_status = TelemetryStatus::NotApplicable;
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
                // Reuse the just-completed install/update/etc.'s own
                // operation_id/command (captured via save_reboot_operation
                // just before that command's own run_command scope ended)
                // rather than leaving `trident_system_reboot` untagged, or
                // minting an unrelated fresh "reboot" identity: the reboot
                // is a direct continuation of that same servicing
                // operation, not an independent one, so telemetry should
                // correlate it back to the same operation_id.
                // run_reboot_command also still fires command_error on a
                // failed reboot -- falling back to a fresh, untagged
                // command if nothing was captured -- matching every other
                // command's error-reporting contract instead of silently
                // dropping this one on the floor.
                if let Err(e) = run_reboot_command(trident::request_reboot_with_wait) {
                    error!("Failed to reboot: {e:?}");
                    return TridentExitCodes::RebootUnsuccessful.into();
                }
            }
        }

        TridentExitCodes::Success.into()
    }
}
