use std::{fs, iter, panic, process::ExitCode};

use anyhow::{Context, Error};
use clap::Parser;
use log::{error, info, LevelFilter, Log};

use osutils::logging::{filter::LogFilter, multilog::MultiLogger};
use trident::{
    agentconfig::AgentConfig,
    cli::{self, Cli, Commands, GetKind, TridentExitCodes},
    init::offline,
    manual_rollback::{self, utils::ManualRollbackRequestKind},
    run_with_captured_operation, run_with_operation, save_reboot_operation, take_reboot_operation,
    validation, BackgroundLog, BackgroundUploader, DataStore, ExitKind, LogForwarder, Logstream,
    OperationSource, TraceStream, Trident, TRIDENT_BACKGROUND_LOG_PATH,
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
                // Determined before any preflight checks below, and used
                // to wrap the *entire* servicing branch (preflight checks,
                // Trident::new, and the actual command) in a single
                // run_with_operation call -- not just the innermost
                // install/update/commit/rollback/rebuild-raid call, as
                // before. That previously left Trident::new (and
                // everything it does, including firing "trident_start")
                // outside any operation context: every event from CLI
                // startup through to just before the actual command ran
                // had no operation_id/command.
                // `Commands::kind()` is total over every variant
                // (including ones that can never actually reach this
                // arm), so it renders the same wire name the old
                // per-variant match did without needing an
                // `unreachable!()` fallback to prove exhaustiveness.
                let kind = args.command.kind();

                // Determined up front, before any preflight checks below,
                // so a missing/nonexistent --config is rejected immediately.
                let config_path = match &args.command {
                    Commands::Update { config, .. } | Commands::Install { config, .. } => {
                        Some(config.clone())
                    }
                    Commands::RebuildRaid { config, .. } => config.clone(),
                    _ => None,
                };
                if let Some(path) = &config_path {
                    if !path.exists() {
                        return run_with_operation(&kind, OperationSource::Cli, || {
                            Err(TridentError::new(InvalidInputError::ReadInputFile {
                                path: path.to_string_lossy().to_string(),
                            }))
                            .message("Config file does not exist")
                        });
                    }
                }

                // Determine this up front, before the pre-warm attach
                // below: a multiboot install may swap to a brand-new
                // temporary datastore inside `Trident::install` (see
                // there), distinct from `datastore_path` here (the
                // existing host's persistent datastore) -- so skip
                // attaching an installation ID from the pre-swap datastore
                // until `install` has settled on which one it actually
                // uses, rather than attaching the existing host's here and
                // having it (and every event emitted before the swap
                // decision) be wrong for the rest of the run. See
                // `new_deferring_installation_id`'s doc comment for the
                // full rationale.
                let defer_installation_id = matches!(
                    args.command,
                    Commands::Install {
                        multiboot: true,
                        ..
                    }
                );

                // Attach this host's installation ID and current servicing
                // ID to the shared TraceStream before run_with_operation
                // below fires command_start: Trident::new (further down,
                // inside the closure) is the usual place installation ID
                // gets attached, but that's too late for command_start,
                // which run_with_operation fires immediately, before the
                // closure even runs. Does not create a *datastore* (see
                // `TraceStream::attach_ids_if_present`) -- silently does
                // nothing if the datastore doesn't exist yet, which is
                // expected for a host's first-ever install. But not purely
                // read-only: on an existing datastore that predates
                // `installation_id`, this can perform a one-time migration
                // write to mint one (see
                // `DataStore::installation_id_or_migrate`). Skips the
                // installation-ID half specifically (but still attaches
                // servicing ID, unaffected by a multiboot swap) when
                // `defer_installation_id` is set, for the same reason
                // `Trident::new_deferring_installation_id` is used below.
                //
                // Load once and reuse the same snapshot for both the
                // pre-warm attach here and the operation closure below:
                // calling `AgentConfig::load()` a second time inside the
                // closure could observe a different `DatastorePath` (e.g. a
                // CIH bootstrap swap between the two reads), leaving the
                // ID cached on `tracestream` here attributed to a
                // different datastore than the one the operation actually
                // runs against.
                let agent_config_result = AgentConfig::load();
                if let Ok(agent_config) = &agent_config_result {
                    tracestream.attach_ids_if_present(
                        agent_config.datastore_path(),
                        defer_installation_id,
                    );
                }

                run_with_operation(&kind, OperationSource::Cli, || {
                    // config_path was already validated (existence-checked)
                    // above. Reuse the same `AgentConfig` snapshot loaded
                    // just above rather than reloading -- see the comment
                    // there.
                    let agent_config = agent_config_result?;
                    // For commands that cannot themselves stage a new
                    // install/update (see
                    // `CommandKind::may_initialize_datastore`), we expect
                    // the datastore to already exist. Update has its own
                    // special handling for the CIH bootstrap scenario
                    // further down.
                    if !kind.may_initialize_datastore() && !agent_config.datastore_path().exists() {
                        return Err(TridentError::new(InvalidInputError::HostNotProvisioned))
                            .message("Datastore file does not exist");
                    }

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
                    // inside `run_with_operation`'s scope), so the reboot
                    // requested below by the caller can be tagged with the
                    // *original* install/update/etc.'s `operation_id`/
                    // `command` instead of a disconnected fresh one -- see
                    // `save_reboot_operation`/`take_reboot_operation`.
                    if matches!(res, Ok(ExitKind::NeedsReboot)) {
                        save_reboot_operation();
                    }

                    res.message(format!("Failed to execute '{}' command", args.command))
                })
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

fn setup_tracing(args: &Cli) -> Result<TraceStream, Error> {
    use tracing_subscriber::{filter, layer::SubscriberExt, Layer};

    let tracestream = TraceStream::default();

    match &args.command {
        Commands::Commit { .. }
        | Commands::Daemon { .. }
        | Commands::GrpcClient { .. }
        | Commands::Install { .. }
        | Commands::RebuildRaid { .. }
        | Commands::Rollback { check: false, .. }
        | Commands::Update { .. } => {
            // As functionality moves to the Daemon, move the journald layer to
            // only be enabled for the Daemon command. Until then, keep it enabled
            // for all commands to ensure we have tracing info in journald for all
            // commands.
            let baseline_tracing = tracing_subscriber::Registry::default().with(
                tracestream
                    .make_trace_sender()
                    .with_filter(filter::LevelFilter::INFO),
            );
            if let Ok(journald_layer) = tracing_journald::layer() {
                tracing::subscriber::set_global_default(
                    baseline_tracing.with(
                        journald_layer
                            .with_syslog_identifier("trident-tracing".to_string())
                            .with_filter(filter::LevelFilter::INFO),
                    ),
                )
                .context("Failed to set global default subscriber")?;
            } else {
                eprintln!("Failed to connect to journald, falling back to tracing without journald support");
                tracing::subscriber::set_global_default(baseline_tracing)
                    .context("Failed to set global default subscriber")?;
            }
        }
        _ => {
            // no op
        }
    }

    Ok(tracestream)
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

    // Initialize the telemetry flow
    let tracestream = setup_tracing(&args);
    if let Err(e) = tracestream {
        // Defer to stderr since logging is not yet initialized.
        eprintln!("Failed to initialize tracing: {e:?}");
        return TridentExitCodes::SetupFailed.into();
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

        trident::server_main(
            log_forwarder,
            *inactivity_timeout,
            socket_path,
            logstream.unwrap(),
            tracestream.unwrap(),
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

        // Run the client command
        trident::client_main(client_args)
    } else {
        // Initialize the loggers
        let logstream = setup_logging(&args, &bg_uploader, iter::empty());
        if let Err(e) = logstream {
            error!("Failed to initialize logging: {e:?}");
            return TridentExitCodes::SetupFailed.into();
        }

        // Invoke Trident
        match run_trident(logstream.unwrap(), tracestream.unwrap(), &args) {
            Ok(ExitKind::Done) => {}
            Err(e) => {
                error!("{e:?}");
                return TridentExitCodes::Failed.into();
            }
            Ok(ExitKind::NeedsReboot) => {
                if let Err(e) = run_with_captured_operation(
                    take_reboot_operation(),
                    trident::request_reboot_with_wait,
                ) {
                    error!("Failed to reboot: {e:?}");
                    return TridentExitCodes::RebootUnsuccessful.into();
                }
            }
        }

        TridentExitCodes::Success.into()
    }
}
