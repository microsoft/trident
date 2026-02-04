use std::sync::mpsc::{self, Receiver};

use anyhow::{Context, Error};
use log::info;
use signal_hook::{
    consts::signal::{SIGINT, SIGQUIT, SIGTERM},
    iterator::Signals,
};
use tokio_util::sync::CancellationToken;

pub struct ShutdownSignals {
    token: CancellationToken,
    exit_rx: Receiver<()>,
}

impl ShutdownSignals {
    /// Returns a child cancellation token that can be used to monitor for
    /// shutdown signals.
    pub fn token(&self) -> CancellationToken {
        self.token.child_token()
    }

    /// Returns a receiver that will be notified when a shutdown signal is
    /// received.
    pub fn exit_receiver(&self) -> &Receiver<()> {
        &self.exit_rx
    }

    /// Sets up handlers for termination signals (SIGTERM, SIGINT, SIGQUIT).
    /// When one of these signals is received, the cancellation token is
    /// cancelled and a notification is sent over the channel.
    pub fn setup_signal_handlers() -> Result<Self, Error> {
        let mut signals =
            Signals::new([SIGTERM, SIGINT, SIGQUIT]).context("Failed to set up signal handlers")?;

        // Set up a channel to notify about received signals.
        let (tx, rx) = mpsc::channel();

        // Create a cancellation token to signal shutdown in async contexts.
        let token = CancellationToken::new();
        let token_child = token.child_token();

        std::thread::spawn(move || {
            for signal in signals.forever() {
                match signal {
                    SIGTERM | SIGINT | SIGQUIT => {
                        // Handle termination signals
                        info!("Received termination signal: {signal}");
                        token.cancel();
                        let _ = tx.send(());
                    }
                    other_signal => {
                        log::warn!("Received unexpected signal: {}", other_signal);
                    }
                }
            }
        });

        Ok(Self {
            token: token_child,
            exit_rx: rx,
        })
    }
}
