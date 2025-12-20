use std::{fs, os::fd::AsRawFd, path::Path, time::Duration};

use anyhow::{bail, Context, Result as AnyhowRes};
use log::{debug, info};
use nix::sys::stat::Mode;
use tokio::{
    net::UnixListener,
    signal::unix::{self, SignalKind},
};
use tokio_stream::wrappers::UnixListenerStream;
use tonic::transport::Server;
use tonic_middleware::MiddlewareFor;

use harpoon::trident_service_server::TridentServiceServer;

use crate::{
    logging::logfwd::LogForwarder,
    server::{activitytracker::ActivityTracker, support::fds},
};

mod activitytracker;
mod support;
mod tridentserver;

use tridentserver::TridentHarpoonServer;

/// Default path for the Trident Unix domain socket. This is used when Trident
/// itself creates the socket when invoked directly, and not as part of a
/// systemd socket invocation.
const DEFAULT_TRIDENT_SOCKET_PATH: &str = "/var/run/trident.sock";

/// Default inactivity timeout in seconds for the ActivityTracker. When fully
/// inactive, meaning there are no ongoing requests or active connections, for
/// this duration, the server will shut down gracefully automatically.
const DEFAULT_INACTIVITY_TIMEOUT_SECS: u64 = 300;

pub async fn server_main(log_fwd: LogForwarder) -> AnyhowRes<()> {
    info!("Starting gRPC server");
    let listener = set_up_listener()?;
    debug!("Trident listening on socket: {:?}", listener.local_addr()?);

    // Set up activity tracker. This will monitor for inactivity and trigger
    // shutdown when the timeout is reached.
    let (activity_tracker, mut shutdown_rx, monitor_token) =
        ActivityTracker::new(Duration::from_secs(DEFAULT_INACTIVITY_TIMEOUT_SECS));

    // Set up signal handler for SIGTERM
    let mut sigterm =
        unix::signal(SignalKind::terminate()).context("Failed to set up SIGTERM handler")?;

    Server::builder()
        .add_service(MiddlewareFor::new(
            TridentServiceServer::new(TridentHarpoonServer::new(log_fwd, activity_tracker.clone())),
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
fn set_up_listener() -> AnyhowRes<UnixListener> {
    // Check for systemd socket activation
    let sd_listener_fds = fds::get_sd_fd_socket_data()
        .context("Failed to get socket data from systemd environment variables")?;

    // If more than one socket fd is passed, bail out.
    if sd_listener_fds.len() > 1 {
        bail!("unexpected: more than one socket passed in LISTEN_FDS");
    }

    // Use the systemd-passed socket if available, otherwise bind to default path
    let listener = if let Some((sd_listener_fd, fd_name)) = sd_listener_fds.into_iter().next() {
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
        fds::get_listener_from_fd(sd_listener_fd)?
    } else {
        debug!("No systemd socket activation detected, binding to default socket path");
        fds::create_unix_socket(DEFAULT_TRIDENT_SOCKET_PATH, Mode::from_bits_truncate(0o600))?
    };

    Ok(listener)
}
