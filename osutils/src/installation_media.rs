use std::fs;

use anyhow::{Context, Error};
use log::{debug, info, warn};

use trident_api::error::{InitializationError, ReportError, TridentError};

use crate::dependencies::Dependency;

/// Represents the boot method used to start the system.
pub enum BootType {
    /// System is running from a RAM disk
    RamDisk,
    /// System is running from live media
    LiveMedia,
    /// System is running from persistent storage
    PersistentStorage,
}

/// Detects how the system was booted by examining `/proc/cmdline` and returns the BootType.
pub fn detect_boot_type() -> Result<BootType, TridentError> {
    let cmdline =
        fs::read_to_string("/proc/cmdline").structured(InitializationError::ReadCmdline)?;

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
pub fn handle_installation_media() -> Result<(), TridentError> {
    info!("Attempting to eject installation media");
    match detect_boot_type()? {
        BootType::RamDisk => {
            if let Err(e) = eject_media() {
                warn!("Failed to eject installation media. Please remove the installation media when the system reboots. Ejection error: {e:?}");
            }
        }
        BootType::LiveMedia => {
            info!("Please remove the installation media when the system reboots");
        }
        BootType::PersistentStorage => {
            debug!("No installation media ejection needed");
        }
    }
    Ok(())
}
