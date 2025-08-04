use anyhow::{Context, Error};
use log::{debug, info, warn};
use std::fs;

use crate::dependencies::Dependency;

pub enum BootType {
    /// System is running from a RAM disk
    RamDisk,
    /// System is running directly from CD-ROM/DVD
    LiveCdrom,
    /// System is running from persistent storage
    PersistentStorage,
}

pub fn detect_boot_type() -> Result<BootType, Error> {
    let cmdline = fs::read_to_string("/proc/cmdline")
        .with_context(|| "Failed to read /proc/cmdline to detect boot type")?;

    if cmdline.contains("root=/dev/ram0") || !cmdline.contains("root=") {
        debug!("RAM disk boot detected");
        Ok(BootType::RamDisk)
    } else if cmdline.contains("root=live:LABEL=CDROM") || cmdline.contains("root=live:") {
        debug!("Live CD-ROM boot detected");
        Ok(BootType::LiveCdrom)
    } else {
        debug!("Persistent storage boot detected");
        Ok(BootType::PersistentStorage)
    }
}

pub fn eject_media() -> Result<(), Error> {
    info!("Attempting to eject installation media");

    let result = Dependency::Eject
        .cmd()
        .args(["--cdrom", "--force"])
        .output_and_check()
        .context("Failed to execute eject command");

    match result {
        Ok(_) => {
            info!("Successfully ejected installation media");
            Ok(())
        }
        Err(e) => {
            warn!("Failed to eject installation media: {e:?}");
            Err(e)
        }
    }
}

pub fn media_ejection() -> Result<(), Error> {
    info!("Attempting to eject installation media");
    match detect_boot_type() {
        Ok(BootType::RamDisk) => {
            if let Err(e) = eject_media() {
                warn!("Failed to eject installation media: {e:?}");
            }
        }
        Ok(BootType::LiveCdrom) => {
            info!("Please remove the installation media when the system reboots.");
        }
        Ok(BootType::PersistentStorage) => {
            debug!("No installation media ejection needed");
        }
        Err(e) => {
            warn!("Unable to detect boot type: {e:?} - skipping installation media ejection");
        }
    }

    Ok(())
}
