use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use log::{debug, error, info, trace, warn};
use tokio::sync::mpsc::{self, Receiver, Sender, UnboundedReceiver, UnboundedSender};
use tokio_util::sync::CancellationToken;

use super::{middleware::ActivityTrackerMiddleware, timer::Timer};

// Tracks active connections and servicing operations, sending inactivity events that can
// trigger an automatic shutdown after a configurable period with no activity.
#[derive(Clone)]
pub(crate) struct ActivityTracker {
    active_connections: Arc<AtomicUsize>,
    active_servicing: Arc<AtomicBool>,
    event_tx: UnboundedSender<EventType>,
}

#[derive(Debug)]
enum EventType {
    NewActivity,
    Inactivity,
}

impl ActivityTracker {
    pub(crate) fn new(timeout: Duration) -> (Self, Receiver<()>, CancellationToken) {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (shutdown_tx, shutdown_rx) = mpsc::channel(1);
        let tracker = Self {
            active_connections: Arc::new(AtomicUsize::new(0)),
            active_servicing: Arc::new(AtomicBool::new(false)),
            event_tx,
        };

        let monitor_token = CancellationToken::new();

        // Start monitoring activity in the background
        tracker
            .clone()
            .monitor_activity(monitor_token.clone(), timeout, event_rx, shutdown_tx);

        (tracker, shutdown_rx, monitor_token)
    }

    pub(crate) fn middleware(&self) -> ActivityTrackerMiddleware {
        ActivityTrackerMiddleware::new(self.clone())
    }

    pub(crate) fn on_connection_start(&self) {
        trace!("Connection started.");
        self.active_connections.fetch_add(1, Ordering::SeqCst);
        self.notify_event(EventType::NewActivity);
    }

    pub(crate) fn on_connection_end(&self) {
        trace!("Connection ended.");
        self.active_connections.fetch_sub(1, Ordering::SeqCst);
        self.notify_event(EventType::Inactivity);
    }

    pub(crate) fn on_servicing_started(&self) {
        trace!("Servicing started.");
        self.active_servicing.store(true, Ordering::SeqCst);
        self.notify_event(EventType::NewActivity);
    }

    pub(crate) fn on_servicing_ended(&self) {
        trace!("Servicing ended.");
        self.active_servicing.store(false, Ordering::SeqCst);
        self.notify_event(EventType::Inactivity);
    }

    pub(crate) fn has_active_connections(&self) -> bool {
        self.active_connections.load(Ordering::SeqCst) > 0
    }

    pub(crate) fn is_servicing_active(&self) -> bool {
        self.active_servicing.load(Ordering::SeqCst)
    }

    fn notify_event(&self, event_type: EventType) {
        if let Err(err) = self.event_tx.send(event_type) {
            warn!(
                "ActivityTracker failed to send event notification (receiver may be dropped): {}",
                err
            );
        }
    }

    /// Monitors activity events and manages the shutdown timer.
    /// When inactivity is detected and there are no active connections or
    /// servicing, the tracker starts a countdown timer with the provided
    /// timeout duration to trigger server shutdown.
    fn monitor_activity(
        self,
        token: CancellationToken,
        timeout: Duration,
        mut event_rx: UnboundedReceiver<EventType>,
        shutdown_tx: Sender<()>,
    ) {
        tokio::spawn(async move {
            let mut timer: Option<Timer> = None;
            loop {
                tokio::select! {
                    _ = token.cancelled() => {
                        break;
                    }

                    // Handle activity event
                    event_type = event_rx.recv() => {
                        trace!("Activity event received: {:?}", event_type);
                        match event_type {
                            Some(EventType::NewActivity) => {
                                // Cancel any existing timer
                                if let Some(t) = timer.take() { t.cancel() }
                            }
                            Some(EventType::Inactivity) => {
                                if self.has_active_connections() || self.is_servicing_active() {
                                    // Still active, do nothing
                                    continue;
                                }

                                info!("No active connections or servicing. Starting shutdown timer...");

                                // Cancel any existing timer
                                if let Some(t) = timer.take() { t.cancel() }

                                // Start a new timer
                                let shutdown_tx_clone = shutdown_tx.clone();
                                timer.replace(Timer::new(timeout, move || {
                                    info!("Shutdown timer expired. Shutting down server...");
                                    if shutdown_tx_clone.try_send(()).is_err() {
                                        error!("Failed to send shutdown signal");
                                    }
                                }));

                            }
                            None => {
                                warn!("Event channel closed unexpectedly.");
                                break;
                            }
                        }
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time;

    #[tokio::test]
    async fn test_activity_tracker_shutdown_on_inactivity() {
        let (tracker, mut shutdown_rx, _token) = ActivityTracker::new(Duration::from_millis(100));

        // Start a connection and servicing
        tracker.on_connection_start();
        tracker.on_servicing_started();

        // End servicing and connection
        tracker.on_servicing_ended();
        tracker.on_connection_end();

        // Wait for shutdown signal
        time::timeout(Duration::from_secs(1), shutdown_rx.recv())
            .await
            .expect("Timeout waiting for shutdown signal");
    }

    #[tokio::test]
    async fn test_activity_tracker_timeout() {
        let (tracker, mut shutdown_rx, _token) = ActivityTracker::new(Duration::from_millis(200));
        // Start a connection
        tracker.on_connection_start();
        // Wait less than the timeout duration
        time::sleep(Duration::from_millis(100)).await;
        // End the connection
        tracker.on_connection_end();

        // Ensure no shutdown signal is received within the timeout duration
        time::timeout(Duration::from_millis(150), shutdown_rx.recv())
            .await
            .expect_err("Unexpected shutdown signal received");

        // Now wait for the shutdown signal after the timeout
        time::timeout(Duration::from_secs(1), shutdown_rx.recv())
            .await
            .expect("Timeout waiting for shutdown signal");
    }

    #[tokio::test]
    async fn test_activity_tracker_no_shutdown_with_active_connection() {
        let (tracker, mut shutdown_rx, _token) = ActivityTracker::new(Duration::from_millis(100));
        // Start a connection
        tracker.on_connection_start();
        // Start servicing
        tracker.on_servicing_started();
        // End servicing
        tracker.on_servicing_ended();

        // Ensure no shutdown signal is received within the timeout duration
        // because there is still an active connection.
        time::timeout(Duration::from_millis(200), shutdown_rx.recv())
            .await
            .expect_err("Unexpected shutdown signal received");

        // Start a new servicing
        tracker.on_servicing_started();
        // End the connection
        tracker.on_connection_end();

        // Ensure no shutdown signal is received within the timeout duration
        // because there is still active servicing.
        time::timeout(Duration::from_millis(200), shutdown_rx.recv())
            .await
            .expect_err("Unexpected shutdown signal received");

        // Now end servicing
        tracker.on_servicing_ended();

        // Now wait for the shutdown signal after the timeout
        time::timeout(Duration::from_secs(1), shutdown_rx.recv())
            .await
            .expect("Timeout waiting for shutdown signal");
    }

    #[tokio::test]
    async fn test_activity_tracker_many_connections() {
        let (tracker, mut shutdown_rx, _token) = ActivityTracker::new(Duration::from_millis(100));

        // Start multiple connections
        for _ in 0..5 {
            tracker.on_connection_start();
        }

        // End some connections
        for _ in 0..3 {
            tracker.on_connection_end();
        }

        // Ensure no shutdown signal is received within the timeout duration
        time::timeout(Duration::from_millis(200), shutdown_rx.recv())
            .await
            .expect_err("Unexpected shutdown signal received");

        // End remaining connections
        for _ in 0..2 {
            tracker.on_connection_end();
        }

        // Now wait for the shutdown signal after the timeout
        time::timeout(Duration::from_secs(1), shutdown_rx.recv())
            .await
            .expect("Timeout waiting for shutdown signal");
    }
}
