//! Trident gRPC server module.

use std::{
    os::{fd::AsRawFd, unix::net::UnixListener as StdUnixListener},
    path::Path,
    process::ExitCode,
    sync::Arc,
    time::Duration,
};

use anyhow::{bail, Context, Result as AnyhowRes};
use log::{debug, error, info};
use nix::sys::stat::Mode;
use tokio::{net::UnixListener, runtime::Builder};
use tokio_stream::wrappers::UnixListenerStream;
use tokio_util::sync::CancellationToken;
use tonic::transport::Server;
use tonic_middleware::MiddlewareFor;

use trident_proto::v1::{
    streaming_service_server::StreamingServiceServer, version_service_server::VersionServiceServer,
};

#[cfg(feature = "grpc-preview")]
use trident_proto::v1preview::{
    commit_service_server::CommitServiceServer, install_service_server::InstallServiceServer,
    rebuild_raid_service_server::RebuildRaidServiceServer,
    rollback_service_server::RollbackServiceServer, status_service_server::StatusServiceServer,
    update_service_server::UpdateServiceServer, validation_service_server::ValidationServiceServer,
};

use crate::{
    agentconfig::AgentConfig,
    cli::TridentExitCodes,
    logging::logfwd::LogForwarder,
    reboot::{self, REBOOT_WAIT_DURATION_SECS},
    ExitKind, Logstream, TraceStream,
};

mod activitytracker;
mod support;
mod tridentserver;

use activitytracker::ActivityTracker;
use support::{
    fds::{self, UnixSocketCleanup},
    signals::ShutdownSignals,
};
use tridentserver::{ServicingManager, TridentHarpoonServer};

/// Default path for the Trident Unix domain socket. This is used when Trident
/// itself creates the socket when invoked directly, and not as part of a
/// systemd socket invocation.
pub const DEFAULT_TRIDENT_SOCKET_PATH: &str = "/run/trident/trident.sock";

/// Default inactivity timeout in seconds for the ActivityTracker. When fully
/// inactive, meaning there are no ongoing requests or active connections, for
/// this duration, the server will shut down gracefully automatically.
pub const DEFAULT_INACTIVITY_TIMEOUT: &str = "300s"; // 5 minutes

/// Main entry point for the Trident gRPC server.
///
/// This function sets up the server, including the listener, activity tracker,
/// and signal handlers, and starts serving incoming requests.
///
/// Any active servicing operation will run on a blocking task thread from
/// Tokio's blocking thread pool, so it will continue running until completion
/// or error even after the server has shut down. This is intentional, as we
/// want servicing operations to complete even if the server is no longer
/// reachable.
///
/// Exit codes:
/// - 0: Normal exit
/// - 1: Setup failed: Tokio runtime or listener setup
/// - 2: Server runtime error
/// - 3: Reboot requested but failed
/// - 4: Agent configuration load failed
pub fn server_main(
    log_fwd: LogForwarder,
    shutdown_timeout: Duration,
    default_socket_path: impl AsRef<Path>,
    logstream: Logstream,
    tracestream: TraceStream,
) -> ExitCode {
    // Start the Tokio runtime
    let Ok(runtime) = Builder::new_multi_thread().enable_all().build() else {
        error!("Failed to create Tokio runtime");
        return TridentExitCodes::SetupFailed.into();
    };

    let (listener, _listener_cleanup) = match set_up_listener(default_socket_path.as_ref()) {
        Ok(res) => res,
        Err(e) => {
            error!("Failed to set up server listener: {e:?}");
            return TridentExitCodes::SetupFailed.into();
        }
    };

    let agent_config = match AgentConfig::load() {
        Ok(cfg) => cfg,
        Err(e) => {
            error!("Failed to load agent configuration: {e:?}");
            return TridentExitCodes::FailedToLoadAgentConfig.into();
        }
    };

    let shutdown_signals = match ShutdownSignals::setup_signal_handlers() {
        Ok(signals) => signals,
        Err(e) => {
            error!("Failed to set up signal handlers: {e:?}");
            return TridentExitCodes::SetupFailed.into();
        }
    };

    let main_task = runtime.block_on(async {
        server_main_inner(
            listener,
            log_fwd,
            shutdown_timeout,
            agent_config,
            logstream,
            tracestream,
            shutdown_signals.token(),
        )
        .await
    });

    // Drop the runtime to clean up resources. This implicitly waits for all
    // spawned blocking tasks to complete.
    drop(runtime);

    let exit_kind = match main_task {
        Ok(exit_kind) => exit_kind,
        Err(e) => {
            error!("Daemon failed: {e:?}");
            return TridentExitCodes::Failed.into();
        }
    };

    match exit_kind {
        // Normal exit
        ExitKind::Done => TridentExitCodes::Success.into(),

        // Reboot requested
        ExitKind::NeedsReboot => reboot(shutdown_signals),
    }
}

fn reboot(signals: ShutdownSignals) -> ExitCode {
    if let Err(e) = reboot::request_reboot() {
        error!("Failed to request reboot: {e:?}");
        return TridentExitCodes::RebootUnsuccessful.into();
    }

    // Wait for either a shutdown signal or the reboot timeout
    if let Err(e) = signals
        .exit_receiver()
        .recv_timeout(Duration::from_secs(REBOOT_WAIT_DURATION_SECS))
    {
        error!("Reboot wait timed out: {e:?}");
        return TridentExitCodes::RebootUnsuccessful.into();
    }

    // A signal was received, exit successfully
    info!("System is rebooting now");
    TridentExitCodes::Success.into()
}

async fn server_main_inner(
    listener: StdUnixListener,
    log_fwd: LogForwarder,
    shutdown_timeout: Duration,
    agent_config: AgentConfig,
    logstream: Logstream,
    tracestream: TraceStream,
    signals_token: CancellationToken,
) -> AnyhowRes<ExitKind> {
    // Ensure the listener is in non-blocking state as required by Tokio
    listener
        .set_nonblocking(true)
        .context("Failed to set listener to non-blocking")?;

    let listener = UnixListener::from_std(listener)
        .context("Failed to create Tokio UnixListener from std listener")?;

    // Set up activity tracker. This will monitor for inactivity and trigger
    // shutdown when the timeout is reached.
    let (activity_tracker, mut shutdown_rx, monitor_token) = ActivityTracker::new(shutdown_timeout);

    let (servicing_manager, exit_token) = ServicingManager::new();

    // The gRPC server implementation for all Trident services. This is wrapped
    // in an Arc since it needs to be shared across the multiple service
    // handlers.
    let harpoon_server = Arc::new(TridentHarpoonServer::new(
        servicing_manager.clone(),
        log_fwd,
        activity_tracker.clone(),
        agent_config,
        logstream,
        tracestream,
    ));

    // Set up the gRPC server with all services, wrapping each in the activity
    // tracker middleware to ensure that any activity on the service resets the
    // inactivity timer.
    let mut router = Server::builder().add_service(MiddlewareFor::new(
        VersionServiceServer::from_arc(harpoon_server.clone()),
        activity_tracker.middleware(),
    ));

    router = router.add_service(MiddlewareFor::new(
        StreamingServiceServer::from_arc(harpoon_server.clone()),
        activity_tracker.middleware(),
    ));

    #[cfg(feature = "grpc-preview")]
    {
        router = router
            .add_service(MiddlewareFor::new(
                CommitServiceServer::from_arc(harpoon_server.clone()),
                activity_tracker.middleware(),
            ))
            .add_service(MiddlewareFor::new(
                InstallServiceServer::from_arc(harpoon_server.clone()),
                activity_tracker.middleware(),
            ))
            .add_service(MiddlewareFor::new(
                RollbackServiceServer::from_arc(harpoon_server.clone()),
                activity_tracker.middleware(),
            ))
            .add_service(MiddlewareFor::new(
                StatusServiceServer::from_arc(harpoon_server.clone()),
                activity_tracker.middleware(),
            ))
            .add_service(MiddlewareFor::new(
                UpdateServiceServer::from_arc(harpoon_server.clone()),
                activity_tracker.middleware(),
            ))
            .add_service(MiddlewareFor::new(
                ValidationServiceServer::from_arc(harpoon_server.clone()),
                activity_tracker.middleware(),
            ))
            .add_service(MiddlewareFor::new(
                RebuildRaidServiceServer::from_arc(harpoon_server.clone()),
                activity_tracker.middleware(),
            ));
    }

    info!(
        "Starting gRPC server listening on: {:?}",
        listener.local_addr()?
    );
    router
        .serve_with_incoming_shutdown(UnixListenerStream::new(listener), async {
            tokio::select! {
                _ = shutdown_rx.recv() => {
                    info!("gRPC server shutdown requested due to inactivity timeout");
                }
                _ = signals_token.cancelled() => {
                    info!("gRPC server shutdown requested by external signal");
                }
                _ = exit_token.cancelled() => {
                    info!("gRPC server shutdown request received from servicing operation");
                }
            }
        })
        .await
        .context("gRPC server failed")?;

    // Cancel activity monitoring
    monitor_token.cancel();

    info!("Server shut down gracefully");

    // Wait on any ongoing servicing operations to complete. This should only
    // block in the relatively uncommon case where the server has exited but
    // there was an ongoing servicing task.
    info!("Waiting for ongoing servicing operations to complete...");
    activity_tracker.wait_on_service_end().await;
    info!("All servicing operations completed");

    Ok(servicing_manager.get_exit_kind().await)
}

/// Sets up the UnixListener for the server, either from a systemd-passed
/// file descriptor or by binding to the default socket path.
fn set_up_listener(
    default_socket_path: impl AsRef<Path>,
) -> AnyhowRes<(StdUnixListener, UnixSocketCleanup)> {
    // Check for systemd socket activation
    let sd_listener_fds = fds::get_sd_fd_socket_data()
        .context("Failed to get socket data from systemd environment variables")?;

    // If more than one socket fd is passed, bail out.
    if sd_listener_fds.len() > 1 {
        bail!("unexpected: more than one socket passed in LISTEN_FDS");
    }

    // Use the systemd-passed socket if available, otherwise bind to default path
    Ok(
        if let Some((sd_listener_fd, fd_name)) = sd_listener_fds.into_iter().next() {
            // Enforce that the fd is a Unix socket to avoid surprises later on like
            // inadvertently listening on a network socket due to a bad config
            // change.
            if !fds::is_unix_socket(sd_listener_fd.as_raw_fd()) {
                bail!(
                    "File descriptor {}[{}] provided by systemd is not a Unix socket",
                    fd_name,
                    sd_listener_fd.as_raw_fd()
                );
            }

            debug!(
                "Activated by systemd socket: listening on file descriptor: {}[{}]",
                fd_name,
                sd_listener_fd.as_raw_fd(),
            );
            (
                StdUnixListener::from(sd_listener_fd),
                UnixSocketCleanup::empty(),
            )
        } else {
            debug!(
                "No systemd socket activation detected, binding to default socket path: {}",
                default_socket_path.as_ref().display()
            );

            let listener =
                fds::create_unix_socket(default_socket_path, Mode::from_bits_truncate(0o600))?;

            let listener_cleanup = UnixSocketCleanup::new(
                listener
                    .local_addr()
                    .context("Failed to get local address of bound socket")?
                    .as_pathname()
                    .context("Failed to get socket path from local address")?
                    .to_owned(),
            );

            (listener, listener_cleanup)
        },
    )
}
