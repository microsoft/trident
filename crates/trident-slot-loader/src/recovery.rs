//! Pending-marker lifecycle and independent update-failure recovery.
//!
//! Recovery never reads the manifest or invokes a mount backend. Loader success clears the marker;
//! it is not a full-boot or native-merge success marker.

use std::{env, fs, io, path::Path};

use anyhow::{Context, Error};
use log::{info, warn};

use crate::{filesystem, tools::Rebooter};

const PENDING: &str = "slot-loader.pending";

pub(super) fn clear_pending(slot: &Path) -> Result<(), Error> {
    if filesystem::remove_regular(slot.join(PENDING))? {
        filesystem::sync_directory(slot)?;
        info!("event=pending_cleared slot_dir={}", slot.display());
    } else {
        info!("event=pending_absent slot_dir={}", slot.display());
    }
    Ok(())
}

pub(super) fn run(slot: &Path, enabled: bool, rebooter: &impl Rebooter) -> Result<(), Error> {
    info!(
        "event=recovery_check slot_dir={} enabled={enabled} failed_unit={:?} service_result={:?}",
        slot.display(),
        env::var_os("MONITOR_UNIT"),
        env::var_os("MONITOR_SERVICE_RESULT")
    );
    if !enabled {
        info!("event=recovery_skipped reason=disabled");
        return Ok(());
    }
    let marker = slot.join(PENDING);
    match fs::symlink_metadata(&marker) {
        Ok(_) => filesystem::regular_file(&marker)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            info!("event=recovery_skipped reason=no_pending_marker");
            return Ok(());
        }
        Err(error) => return Err(error).context("inspecting recovery marker"),
    }
    warn!(
        "event=reboot_requested reason=pending_update_loader_failure marker={}",
        marker.display()
    );
    rebooter
        .reboot()
        .context("requesting pending-update recovery reboot")
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{cell::Cell, os::unix::fs::symlink};

    struct MockRebooter(Cell<usize>);

    impl Rebooter for MockRebooter {
        fn reboot(&self) -> Result<(), Error> {
            self.0.set(self.0.get() + 1);
            Ok(())
        }
    }

    #[test]
    fn recovery_is_marker_gated_and_independent_of_manifest_parsing() {
        let root = tempfile::tempdir().unwrap();
        let slot = root.path();
        let rebooter = MockRebooter(Cell::new(0));
        fs::write(slot.join("slot-loader.json"), "malformed").unwrap();
        run(slot, true, &rebooter).unwrap();
        assert_eq!(rebooter.0.get(), 0);
        fs::write(slot.join(PENDING), "").unwrap();
        run(slot, false, &rebooter).unwrap();
        assert_eq!(rebooter.0.get(), 0);
        run(slot, true, &rebooter).unwrap();
        assert_eq!(rebooter.0.get(), 1);
        assert!(slot.join(PENDING).exists());
        fs::remove_file(slot.join(PENDING)).unwrap();
        symlink("slot-loader.json", slot.join(PENDING)).unwrap();
        run(slot, true, &rebooter).unwrap_err();
        assert_eq!(rebooter.0.get(), 1);
    }
}
