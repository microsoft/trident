use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Error};

/// Read-only access to one block device's sysfs attributes.
pub struct SysfsBlockDevice {
    /// Path to the device's `/sys/class/block` entry.
    path: PathBuf,
}

impl SysfsBlockDevice {
    /// Uses an existing sysfs entry path without reading or validating it yet.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Reads the kernel's read-only flag.
    pub fn is_read_only(&self) -> Result<bool, Error> {
        read_flag(&self.path.join("ro"))
    }

    /// Reads the kernel's removable-media flag.
    pub fn is_removable(&self) -> Result<bool, Error> {
        read_flag(&self.path.join("removable"))
    }

    /// Checks whether an MMC-family device reports type `MMC` rather than `SD`.
    ///
    /// Call only for MMC-family devices; a missing type attribute is an error.
    pub fn is_emmc(&self) -> Result<bool, Error> {
        let path = self.path.join("device/type");
        let media_type = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read MMC card type '{}'", path.display()))?;
        Ok(media_type.trim() == "MMC")
    }

    /// Reads the logical sector size in bytes without imposing compatibility policy.
    pub fn logical_sector_size(&self) -> Result<u32, Error> {
        let path = self.path.join("queue/logical_block_size");
        fs::read_to_string(&path)
            .with_context(|| format!("Failed to read logical sector size '{}'", path.display()))?
            .trim()
            .parse()
            .with_context(|| format!("Invalid logical sector size in '{}'", path.display()))
    }

    /// Checks whether another block device holds this device.
    ///
    /// Checks only this entry, not descendants. Inspection failures return errors.
    pub fn has_holders(&self) -> Result<bool, Error> {
        let path = self.path.join("holders");
        let mut holders = fs::read_dir(&path).with_context(|| {
            format!(
                "Failed to inspect block-device holders '{}'",
                path.display()
            )
        })?;
        Ok(holders
            .next()
            .transpose()
            .with_context(|| format!("Failed to read block-device holders '{}'", path.display()))?
            .is_some())
    }
}

/// Reads a sysfs boolean, accepting only `0` or `1`.
fn read_flag(path: &Path) -> Result<bool, Error> {
    let value = fs::read_to_string(path)
        .with_context(|| format!("Failed to read sysfs flag '{}'", path.display()))?;
    match value.trim() {
        "0" => Ok(false),
        "1" => Ok(true),
        value => anyhow::bail!("Invalid flag '{value}' in '{}'", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_flags_and_rejects_missing_or_malformed_values() {
        let directory = tempfile::tempdir().unwrap();
        let device = SysfsBlockDevice::new(directory.path());
        assert!(device.is_read_only().unwrap_err().is::<std::io::Error>());
        assert!(device.is_removable().unwrap_err().is::<std::io::Error>());
        for value in ["0\n", "1\n"] {
            fs::write(directory.path().join("ro"), value).unwrap();
            fs::write(directory.path().join("removable"), value).unwrap();
            assert_eq!(device.is_read_only().unwrap(), value == "1\n");
            assert_eq!(device.is_removable().unwrap(), value == "1\n");
        }
        fs::write(directory.path().join("ro"), "2").unwrap();
        fs::write(directory.path().join("removable"), "").unwrap();
        assert!(device.is_read_only().is_err());
        assert!(device.is_removable().is_err());
    }

    #[test]
    fn reads_mmc_type_and_preserves_probe_errors() {
        let directory = tempfile::tempdir().unwrap();
        let device = SysfsBlockDevice::new(directory.path());
        assert!(device.is_emmc().unwrap_err().is::<std::io::Error>());
        fs::create_dir(directory.path().join("device")).unwrap();
        for (value, expected) in [("MMC\n", true), ("SD\n", false)] {
            fs::write(directory.path().join("device/type"), value).unwrap();
            assert_eq!(device.is_emmc().unwrap(), expected);
        }
    }

    #[test]
    fn reads_sector_size_without_selection_policy_and_preserves_parse_errors() {
        let directory = tempfile::tempdir().unwrap();
        let device = SysfsBlockDevice::new(directory.path());
        assert!(device
            .logical_sector_size()
            .unwrap_err()
            .is::<std::io::Error>());
        fs::create_dir(directory.path().join("queue")).unwrap();
        let path = directory.path().join("queue/logical_block_size");
        for size in [512, 4096, 8192] {
            fs::write(&path, format!("{size}\n")).unwrap();
            assert_eq!(device.logical_sector_size().unwrap(), size);
        }
        fs::write(&path, "invalid").unwrap();
        assert!(device
            .logical_sector_size()
            .unwrap_err()
            .is::<std::num::ParseIntError>());
    }

    #[test]
    fn checks_holders_without_hiding_inspection_errors() {
        let directory = tempfile::tempdir().unwrap();
        let device = SysfsBlockDevice::new(directory.path());
        assert!(device.has_holders().unwrap_err().is::<std::io::Error>());
        let holders = directory.path().join("holders");
        fs::create_dir(&holders).unwrap();
        assert!(!device.has_holders().unwrap());
        fs::write(holders.join("dm-0"), []).unwrap();
        assert!(device.has_holders().unwrap());
    }
}
