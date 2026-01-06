//! Trident gRPC server module.

use std::{
    os::{fd::AsRawFd, unix::net::UnixListener as StdUnixListener},
    path::Path,
    process::ExitCode,
    time::Duration,
};

use anyhow::{bail, Context, Result as AnyhowRes};
use log::{debug, error, info};
use nix::sys::stat::Mode;
use tokio::{
    net::UnixListener,
    runtime::Builder,
    signal::unix::{self, SignalKind},
};
use tokio_stream::wrappers::UnixListenerStream;
use tonic::transport::Server;
use tonic_middleware::MiddlewareFor;

use harpoon::trident_service_server::TridentServiceServer;

use crate::{
    agentconfig::AgentConfig,
    logging::logfwd::LogForwarder,
    server::{activitytracker::ActivityTracker, fds::UnixSocketCleanup, support::fds},
    Logstream, TraceStream, TRIDENT_VERSION,
};

mod activitytracker;
mod support;
mod tridentserver;

use tridentserver::TridentHarpoonServer;

/// Default path for the Trident Unix domain socket. This is used when Trident
/// itself creates the socket when invoked directly, and not as part of a
/// systemd socket invocation.
pub const DEFAULT_TRIDENT_SOCKET_PATH: &str = "/var/run/trident.sock";

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
pub fn server_main(
    log_fwd: LogForwarder,
    shutdown_timeout: Duration,
    default_socket_path: impl AsRef<Path>,
    logstream: Logstream,
    tracestream: TraceStream,
) -> ExitCode {
    // Log Trident version on startup.
    info!("Trident version: {}", TRIDENT_VERSION);

    // Start the Tokio runtime
    let Ok(runtime) = Builder::new_multi_thread().enable_all().build() else {
        error!("Failed to create Tokio runtime");
        return ExitCode::from(1);
    };

    let (listener, _listener_cleanup) = match set_up_listener(default_socket_path.as_ref()) {
        Ok(res) => res,
        Err(e) => {
            error!("Failed to set up server listener: {e:?}");
            return ExitCode::from(1);
        }
    };

    let agent_config = match AgentConfig::load() {
        Ok(cfg) => cfg,
        Err(e) => {
            error!("Failed to load agent configuration: {e:?}");
            return ExitCode::from(3);
        }
    };

    if let Err(e) = runtime.block_on(async {
        server_main_inner(
            listener,
            log_fwd,
            shutdown_timeout,
            agent_config,
            logstream,
            tracestream,
        )
        .await
    }) {
        error!("Daemon failed: {e:?}");
        return ExitCode::from(2);
    }

    ExitCode::SUCCESS
}

async fn server_main_inner(
    listener: StdUnixListener,
    log_fwd: LogForwarder,
    shutdown_timeout: Duration,
    agent_config: AgentConfig,
    logstream: Logstream,
    tracestream: TraceStream,
) -> AnyhowRes<()> {
    // Ensure the listener is in non-blocking state as required by Tokio
    listener
        .set_nonblocking(true)
        .context("Failed to set listener to non-blocking")?;

    let listener = UnixListener::from_std(listener)
        .context("Failed to create Tokio UnixListener from std listener")?;

    // Set up activity tracker. This will monitor for inactivity and trigger
    // shutdown when the timeout is reached.
    let (activity_tracker, mut shutdown_rx, monitor_token) = ActivityTracker::new(shutdown_timeout);

    // Set up signal handler for SIGTERM
    let mut sigterm =
        unix::signal(SignalKind::terminate()).context("Failed to set up SIGTERM handler")?;

    info!(
        "Starting gRPC server listening on: {:?}",
        listener.local_addr()?
    );
    Server::builder()
        .add_service(MiddlewareFor::new(
            TridentServiceServer::new(TridentHarpoonServer::new(
                log_fwd,
                activity_tracker.clone(),
                agent_config,
                logstream,
                tracestream,
            )),
            activity_tracker.middleware(),
        ))
        .serve_with_incoming_shutdown(UnixListenerStream::new(listener), async {
            tokio::select! {
                _ = shutdown_rx.recv() => {
                    info!("Shutdown signal received");
                }
                _ = tokio::signal::ctrl_c() => {
                    info!("Ctrl-C received, shutting down");
                }
                _ = sigterm.recv() => {
                    info!("SIGTERM received, shutting down");
                }
            }
        })
        .await
        .context("gRPC server failed")?;

    // Cancel activity monitoring
    monitor_token.cancel();

    info!("Server shut down gracefully");

    // NOTE:
    //
    // Any active servicing operation will run on a blocking task thread from
    // Tokio's blocking thread pool, so it will continue running until
    // completion or error even after the server has shut down. This is
    // intentional, as we want servicing operations to complete even if the
    // server is no longer reachable.

    Ok(())
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
