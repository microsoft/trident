use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{bail, ensure, Context, Error};
use trident_api::{
    config::{Disk, HostConfiguration},
    BlockDeviceId,
};

use crate::{
    exe::RunAndCheck,
    lsblk::{self, BlockDeviceType},
};

pub struct ResolvedDisk {
    /// Shortcut to the disk id.
    pub id: BlockDeviceId,

    /// Reference to the disk configuration.
    pub spec: Disk,

    /// Path to the disk in /dev.
    /// Will probably be used in the future.
    #[allow(dead_code)]
    pub dev_path: PathBuf,

    /// Path to the disk in /dev/disk/by-path.
    pub bus_path: PathBuf,
}

/// Resolves the disk paths in the host configuration to their real paths in
/// /dev.
pub fn get_resolved_disks(host_config: &HostConfiguration) -> Result<Vec<ResolvedDisk>, Error> {
    host_config
        .storage
        .disks
        .iter()
        .map(|disk| {
            // Find the real path of the disk in /dev.
            let dev_path = disk.device.canonicalize().context(format!(
                "Failed to lookup device '{}'",
                disk.device.display()
            ))?;

            // Find the symlink path of the disk in /dev/disk/by-path.
            let bus_path = block_device_by_path(&dev_path).context(format!(
                "Failed to find bus path of '{}'",
                dev_path.display()
            ))?;

            Ok(ResolvedDisk {
                id: disk.id.clone(),
                spec: disk.clone(),
                dev_path,
                bus_path,
            })
        })
        .collect::<Result<Vec<_>, Error>>()
        .context("Failed to resolve disk paths")
}

/// Retrieves the symlink for a given block device in '/dev/disk/by-path'.
pub fn block_device_by_path(path: impl AsRef<Path>) -> Result<PathBuf, Error> {
    find_symlink_for_target(path.as_ref(), Path::new("/dev/disk/by-path"))
}

/// Returns the path of the first symlink in directory whose canonical path is target.
pub fn find_symlink_for_target(
    target: impl AsRef<Path>,
    directory: impl AsRef<Path>,
) -> Result<PathBuf, Error> {
    // Ensure that target path is canonicalized
    let target_canonicalized = target.as_ref().canonicalize().context(format!(
        "Failed to canonicalize target path '{}'",
        target.as_ref().display()
    ))?;

    fs::read_dir(directory.as_ref())?
        .flatten()
        .filter(|f| {
            f.file_type()
                .ok()
                .map(|t| t.is_symlink())
                .unwrap_or_default()
        })
        .map(|entry| entry.path())
        .filter(|path| {
            path.canonicalize()
                .map(|p| target_canonicalized == p)
                .unwrap_or_default()
        })
        .min()
        .context(format!(
            "Failed to find symlink for '{}' in directory '{}'",
            target.as_ref().display(),
            directory.as_ref().display()
        ))
}

/// Get the canonicalized path of a disk for a given partition.
pub fn get_disk_for_partition(partition: impl AsRef<Path>) -> Result<PathBuf, Error> {
    let partition_block_device = lsblk::run(partition.as_ref()).with_context(|| {
        format!(
            "Failed to get partition metadata for '{}'",
            partition.as_ref().display(),
        )
    })?;

    ensure!(
        partition_block_device.blkdev_type == BlockDeviceType::Partition,
        "Device '{}' is not a partition",
        partition.as_ref().display()
    );

    partition_block_device.parent_kernel_name.context(format!(
        "Failed to get disk for partition: {:?}, pk_name not found",
        partition.as_ref().display()
    ))
}

/// Check if a device can be stopped. A device can be stopped if it only uses
/// disks that are part of the Host Configuration.
///
/// Returns true if the device can be stopped, false if it should not be
/// touched. Returns an error if the device has underlying disks some of which
/// are part of HC and some are not.
pub fn can_stop_pre_existing_device(
    used_disks: &HashSet<PathBuf>,
    hc_disks: &HashSet<PathBuf>,
) -> Result<bool, Error> {
    let symmetric_diff: HashSet<_> = used_disks.symmetric_difference(hc_disks).cloned().collect();

    if used_disks.is_disjoint(hc_disks) {
        // Device does not have any of its underlying disks mentioned in HostConfig, we should not touch it
        Ok(false)
    } else if symmetric_diff.is_empty() || used_disks.is_subset(hc_disks) {
        // Device's underlying disks are all part of HostConfig, we can unmount and stop the RAID
        return Ok(true);
    } else {
        // Device has underlying disks that are not part of HostConfig, we cannot touch it, abort
        bail!(
            "A device has underlying disks that are not part of Host Configuration. Used disks: {:?}, Host Configuration disks: {:?}",
            used_disks, hc_disks
        );
    }
}

/// Force kernel to re-read the partition table of a disk with partx.
///
/// This function has no built in safety checking. The path must be:
///
/// - A valid block device.
/// - If a disk, it must contain a partition table with at least one partition.
pub fn partx_update(disk: impl AsRef<Path>) -> Result<(), Error> {
    Command::new("partx")
        .arg("--update")
        .arg(disk.as_ref())
        .run_and_check()
        .with_context(|| {
            format!(
                "Failed to re-read partition table for disk '{}'",
                disk.as_ref().display()
            )
        })
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_can_stop_pre_existing_device() -> Result<(), Error> {
        let raid_disks: HashSet<PathBuf> = ["/dev/sda".into(), "/dev/sdb".into()].into();
        let trident_disks: HashSet<PathBuf> = ["/dev/sda".into(), "/dev/sdb".into()].into();
        let trident_disks2: HashSet<PathBuf> = ["/dev/sdb".into(), "/dev/sdc".into()].into();
        let trident_disks3: HashSet<PathBuf> = ["/dev/sdc".into(), "/dev/sdd".into()].into();
        let trident_disks4: HashSet<PathBuf> =
            ["/dev/sda".into(), "/dev/sdb".into(), "/dev/sdc".into()].into();

        // No overlapping disks, should not touch
        let overlap = can_stop_pre_existing_device(&raid_disks, &trident_disks3)?;
        assert!(!overlap);

        // Fully overlapping disks, should stop
        let overlap = can_stop_pre_existing_device(&raid_disks, &trident_disks)?;
        assert!(overlap);

        // Partially overlapping disks, cannot touch, error.
        let overlap = can_stop_pre_existing_device(&raid_disks, &trident_disks2);
        assert!(overlap.is_err());

        // Trident disks are a superset of RAID disks, we can stop
        let overlap = can_stop_pre_existing_device(&raid_disks, &trident_disks4)?;
        assert!(overlap);

        Ok(())
    }

    #[test]
    fn test_find_symlink_for_target() {
        let temp_dir = tempfile::tempdir().unwrap();
        let target = temp_dir.path().canonicalize().unwrap();
        let symlink = temp_dir.path().join("symlink");
        std::os::unix::fs::symlink(&target, &symlink).unwrap();
        assert_eq!(
            find_symlink_for_target(&target, temp_dir.path()).unwrap(),
            symlink
        );

        // Pick the first symlink if there are multiple
        let symlink = temp_dir.path().join("asymlink");
        std::os::unix::fs::symlink(&target, &symlink).unwrap();
        assert_eq!(
            find_symlink_for_target(&target, temp_dir.path()).unwrap(),
            symlink
        );
    }

    #[test]
    fn test_find_symlink_for_target_fail_no_symlink() {
        // Return error if no symlink found
        let temp_dir = tempfile::tempdir().unwrap();
        let target = temp_dir.path().canonicalize().unwrap();
        let temp_dir2 = tempfile::tempdir().unwrap();
        assert_eq!(
            find_symlink_for_target(&target, temp_dir2.path())
                .unwrap_err()
                .to_string(),
            format!(
                "Failed to find symlink for '{}' in directory '{}'",
                target.display(),
                temp_dir2.path().display()
            )
        );
    }

    #[test]
    fn test_find_symlink_for_target_fail_bad_target() {
        // Return error if target path is bad
        let target = Path::new("/bad-target-path");
        let temp_dir = tempfile::tempdir().unwrap();
        assert_eq!(
            find_symlink_for_target(target, temp_dir.path())
                .unwrap_err()
                .to_string(),
            format!("Failed to canonicalize target path '{}'", target.display())
        );
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;

    use pytest_gen::functional_test;

    #[functional_test]
    fn test_get_disk_for_partition() {
        let partition = Path::new("/dev/sda1");
        let disk = get_disk_for_partition(partition).unwrap();
        assert_eq!(disk, Path::new("/dev/sda"));

        let partition = Path::new("/dev/disk/by-path/pci-0000:00:1f.2-ata-2.0-part1");
        let disk = get_disk_for_partition(partition).unwrap();
        assert_eq!(disk, Path::new("/dev/sda"));

        let partition = Path::new("/dev/sdc1");
        assert_eq!(
            get_disk_for_partition(partition).unwrap_err().to_string(),
            "Failed to get partition metadata for '/dev/sdc1'",
        );
    }
}
