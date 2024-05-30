use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Error};

use crate::lsblk;

/// Returns the path of the first symlink in directory whose canonical path is target.
pub fn find_symlink_for_target(target: &Path, directory: &Path) -> Result<PathBuf, Error> {
    // Ensure that target path is canonicalized
    let target_canonicalized = target.canonicalize().context(format!(
        "Failed to canonicalize target path '{}'",
        target.display()
    ))?;

    fs::read_dir(directory)?
        .flatten()
        .filter_map(|f| {
            if let Ok(target_path) = f.path().canonicalize() {
                if target_path == target_canonicalized {
                    return Some(f.path());
                }
            }
            None
        })
        .min()
        .context(format!("Failed to find symlink for '{}'", target.display()))
}

/// Get the canonicalized path of a disk for a given partition.
pub fn get_disk_for_partition(partition: &Path) -> Result<PathBuf, Error> {
    let partition_block_device =
        lsblk::run(partition).context("Failed to get partition metadata")?;

    let parent_kernel_name =
        &partition_block_device
            .parent_kernel_name
            .as_ref()
            .context(format!(
                "Failed to get disk for partition: {:?}, pk_name not found",
                partition
            ))?;

    Ok(PathBuf::from(parent_kernel_name))
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
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;
    use pytest_gen::functional_test;

    #[functional_test(feature = "helpers")]
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

    #[functional_test(feature = "helpers", negative = true)]
    fn test_find_symlink_for_target_fail_no_symlink() {
        // Return error if no symlink found
        let temp_dir = tempfile::tempdir().unwrap();
        let target = temp_dir.path().canonicalize().unwrap();
        let temp_dir2 = tempfile::tempdir().unwrap();
        assert_eq!(
            find_symlink_for_target(&target, temp_dir2.path())
                .unwrap_err()
                .to_string(),
            format!("Failed to find symlink for '{}'", target.display())
        );
    }

    #[functional_test(feature = "helpers", negative = true)]
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
            "Failed to get partition metadata"
        );
    }
}
