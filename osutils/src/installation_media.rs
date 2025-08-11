use std::fs;

use anyhow::{Context, Error};
use log::{debug, info, warn};

use crate::dependencies::Dependency;

/// Enum representing the type of boot the system is running.
pub enum BootType {
    /// System is running from a RAM disk
    RamDisk,
    /// System is running from live media
    LiveMedia,
    /// System is running from persistent storage
    PersistentStorage,
}

/// Detects how the system was booted by examining `/proc/cmdline` and returns the BootType.
pub fn detect_boot_type() -> Result<BootType, Error> {
    let cmdline = fs::read_to_string("/proc/cmdline")
        .with_context(|| "Failed to read /proc/cmdline to detect boot type")?;

    if cmdline.contains("root=/dev/ram0") || !cmdline.contains("root=") {
        debug!("RAM disk boot detected");
        Ok(BootType::RamDisk)
    } else if cmdline.contains("root=live:") {
        debug!("Live media boot detected");
        Ok(BootType::LiveMedia)
    } else {
        debug!("Persistent storage boot detected");
        Ok(BootType::PersistentStorage)
    }
}

/// Ejects the installation media by using the eject command.
fn eject_media() -> Result<(), Error> {
    info!("Attempting to eject installation media");
    Dependency::Eject
        .cmd()
        .args(["--cdrom", "--force"])
        .run_and_check()
        .context("Failed to execute eject command")
}

/// Handles installation media cleanup for clean install based on the BootType before rebooting.
/// Ejects for RAM disk, shows message for live media, and does nothing for persistent storage.
pub fn handle_installation_media() -> Result<(), Error> {
    info!("Attempting to eject installation media");
    match detect_boot_type() {
        Ok(BootType::RamDisk) => {
            if let Err(e) = eject_media() {
                warn!("Failed to eject installation media. Please remove the installation media when the system reboots. Ejection error: {e:?}");
            }
        }
        Ok(BootType::LiveMedia) => {
            info!("Please remove the installation media when the system reboots");
        }
        Ok(BootType::PersistentStorage) => {
            debug!("No installation media ejection needed");
        }
        Err(e) => {
            return Err(e).context("Unable to detect boot type for installation media ejection");
        }
    }

    Ok(())
}
