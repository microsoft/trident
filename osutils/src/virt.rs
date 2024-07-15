//! This module contains helper functions for working with virtualized environments.

/// Does a best-effort check to determine whether we are running in a QEMU VM.
/// Defaults to false when we couldn't find any evidence of QEMU.
///
/// Checks:
///
/// - DMI information (sys_vendor) for "QEMU".
/// - All block devices for "QEMU" in their model.
pub fn is_qemu() -> bool {
    // Check DMI first, as it's more reliable.
    if std::fs::read_to_string("/sys/class/dmi/id/sys_vendor")
        .map(|s| s.contains("QEMU"))
        .unwrap_or_default()
    {
        return true;
    }

    // Do a best-effort check for QEMU block devices.
    for entry in std::fs::read_dir("/sys/class/block")
        .ok()
        .into_iter()
        .flatten()
    {
        // Try to read the entry, but ignore it if it fails.
        let Ok(entry) = entry else {
            continue;
        };

        if std::fs::read_to_string(entry.path().join("device/model"))
            .map(|s| s.contains("QEMU"))
            .unwrap_or_default()
        {
            return true;
        }
    }

    // We have no more ways to check, so we assume it's not QEMU.
    false
}
