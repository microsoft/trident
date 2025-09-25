use std::path::{Path, PathBuf};

use crate::mdadm;

/// Stop the RAID array if it exists.
pub fn stop_if_exists(raid_path: impl AsRef<Path>) {
    if raid_path.as_ref().exists() {
        mdadm::stop(raid_path.as_ref()).unwrap();
    }
}

/// Verify that the RAID array was created on the specified devices.
pub fn verify_raid_creation(raid_path: impl AsRef<Path>, devices: Vec<PathBuf>) {
    let raid_devices = mdadm::detail(raid_path.as_ref()).unwrap();
    // Check if the RAID array was created on the specified devices
    assert_eq!(raid_devices.devices.len(), devices.len());
    for (i, device) in devices.iter().enumerate() {
        assert_eq!(&raid_devices.devices[i], device);
    }
}
