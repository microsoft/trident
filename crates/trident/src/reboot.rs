use std::{thread, time::Duration};

use log::{error, info};

use osutils::dependencies::{Dependency, DependencyError};
use trident_api::error::{ReportError, ServicingError, TridentError};

pub const REBOOT_WAIT_DURATION_SECS: u64 = 600; // 10 minutes

/// Calls `request_reboot` and waits for a fixed duration to allow the reboot to
/// occur. If the system is still running after the wait, an error is returned.
pub fn request_reboot_with_wait() -> Result<(), TridentError> {
    request_reboot().structured(ServicingError::Reboot)?;

    thread::sleep(Duration::from_secs(REBOOT_WAIT_DURATION_SECS));
    error!(
        "Waited for reboot for {REBOOT_WAIT_DURATION_SECS} seconds, but nothing happened, aborting"
    );
    Err(TridentError::new(ServicingError::RebootTimeout))
}

/// Issues a reboot request to systemd after a filesystem sync.
pub fn request_reboot() -> Result<(), Box<DependencyError>> {
    // Sync all writes to the filesystem.
    info!("Syncing filesystem");
    nix::unistd::sync();

    // This trace event will be used with the trident_start event to track the
    // total time taken for the reboot
    tracing::info!(metric_name = "trident_system_reboot");
    info!("Requesting reboot");
    Dependency::Systemctl
        .cmd()
        .env("SYSTEMD_IGNORE_CHROOT", "true")
        .arg("reboot")
        .run_and_check()?;

    // IMPORTANT: This message is used by E2E A/B update tests to validate that
    // a reboot was requested. Do not change or remove without updating the
    // tests!
    info!("Rebooting system");

    Ok(())
}
