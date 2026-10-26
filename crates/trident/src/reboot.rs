use std::{thread, time::Duration};

use log::{debug, error, info};

use osutils::dependencies::{Dependency, DependencyError};
use trident_api::error::{ReportError, ServicingError, TridentError};

pub const REBOOT_WAIT_DURATION_SECS: u64 = 600; // 10 minutes

/// Calls `request_reboot` and waits for a fixed duration to allow the reboot to
/// occur. If the system is still running after the wait, an error is returned.
pub fn request_reboot_with_wait() -> Result<(), TridentError> {
    request_reboot_with_wait_and_flush(|| {})
}

/// Drains telemetry after the reboot metric, before systemd can terminate the process.
pub fn request_reboot_with_wait_and_flush(
    before_reboot: impl FnOnce(),
) -> Result<(), TridentError> {
    request_reboot_with_flush(before_reboot).structured(ServicingError::Reboot)?;

    thread::sleep(Duration::from_secs(REBOOT_WAIT_DURATION_SECS));
    error!(
        "Waited for reboot for {REBOOT_WAIT_DURATION_SECS} seconds, but nothing happened, aborting"
    );
    Err(TridentError::new(ServicingError::RebootTimeout))
}

/// Issues a reboot request to systemd after a filesystem sync.
pub fn request_reboot() -> Result<(), Box<DependencyError>> {
    request_reboot_with_flush(|| {})
}

fn request_reboot_with_flush(before_reboot: impl FnOnce()) -> Result<(), Box<DependencyError>> {
    // Sync all writes to the filesystem.
    info!("Syncing filesystem");
    nix::unistd::sync();

    request_reboot_inner(before_reboot, || {
        Dependency::Systemctl
            .cmd()
            .env("SYSTEMD_IGNORE_CHROOT", "true")
            .arg("reboot")
            .run_and_check()
            .map(|_| ())
    })
}

fn request_reboot_inner(
    before_reboot: impl FnOnce(),
    request: impl FnOnce() -> Result<(), Box<DependencyError>>,
) -> Result<(), Box<DependencyError>> {
    // This trace event will be used with the trident_start event to track the
    // total time taken for the reboot
    tracing::info!(metric_name = "trident_system_reboot");
    before_reboot();

    // IMPORTANT: This message is used by E2E A/B update tests to validate that
    // a reboot was requested. Do not change or remove without updating the
    // test constant in:
    // tools/storm/utils/trident/trident.go:REBOOTING_LOG_MESSAGE
    debug!("Requesting reboot");

    request()?;

    info!("Rebooting system");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{
        cell::Cell,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
    };

    use tracing::{field::Visit, Event, Subscriber};
    use tracing_subscriber::{layer::Context, prelude::*, Layer, Registry};

    struct RebootMetric(Arc<AtomicBool>);

    impl<S: Subscriber> Layer<S> for RebootMetric {
        fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
            struct Visitor<'a>(&'a AtomicBool);
            impl Visit for Visitor<'_> {
                fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
                    if field.name() == "metric_name" && value == "trident_system_reboot" {
                        self.0.store(true, Ordering::SeqCst);
                    }
                }

                fn record_debug(&mut self, _: &tracing::field::Field, _: &dyn std::fmt::Debug) {}
            }
            event.record(&mut Visitor(&self.0));
        }
    }

    #[test]
    fn test_reboot_metric_is_flushed_before_request() {
        let metric_seen = Arc::new(AtomicBool::new(false));
        let flushed = Cell::new(false);
        let subscriber = Registry::default().with(RebootMetric(metric_seen.clone()));
        tracing::subscriber::with_default(subscriber, || {
            request_reboot_inner(
                || {
                    assert!(metric_seen.load(Ordering::SeqCst));
                    flushed.set(true);
                },
                || {
                    assert!(flushed.get());
                    Ok(())
                },
            )
            .unwrap();
        });
        assert!(flushed.get());
    }
}
