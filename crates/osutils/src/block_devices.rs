use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, ensure, Context, Error};
use log::{debug, trace};

use trident_api::{
    config::{Disk, HostConfiguration},
    constants::{PROC_MOUNTINFO_PATH, ROOT_MOUNT_POINT_PATH},
    error::{InternalError, ReportError, TridentError},
    BlockDeviceId,
};

use crate::{
    container,
    dependencies::Dependency,
    lsblk::{self, BlockDeviceType},
    sfdisk::SfDisk,
    tabfile,
};

pub struct ResolvedDisk {
    /// Shortcut to the disk id.
    pub id: BlockDeviceId,

    /// Reference to the disk configuration.
    pub spec: Disk,

    /// Path to the disk in /dev.
    pub dev_path: PathBuf,
}

/// Resolves the disk paths in the Host Configuration to their real paths in `/dev`.
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

            Ok(ResolvedDisk {
                id: disk.id.clone(),
                spec: disk.clone(),
                dev_path,
            })
        })
        .collect::<Result<Vec<_>, Error>>()
        .context("Failed to resolve disk paths")
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
    let partition_block_device = lsblk::get(partition.as_ref()).with_context(|| {
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

/// Force kernel to re-read the partition table of a disk with partx.
///
/// This function has no built in safety checking. The path must be:
///
/// - A valid block device.
/// - If a disk, it must contain a partition table with at least one partition.
pub fn partx_update(disk: impl AsRef<Path>) -> Result<(), Error> {
    Dependency::Partx
        .cmd()
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

/// Gets the partition number of the given partition UUID on the specified disk.
///
/// This function takes the path to the disk and the partition UUID path, then returns the number of
/// the partition that matches the provided UUID.
///
pub fn get_partition_number(
    disk_path: impl AsRef<Path>,
    part_uuid_path: impl AsRef<Path>,
) -> Result<u32, Error> {
    let disk_information = SfDisk::get_info(disk_path.as_ref()).context(format!(
        "Failed to get information for disk '{}'",
        disk_path.as_ref().display()
    ))?;

    for (index, partition) in disk_information.partitions.iter().enumerate() {
        if partition.path_by_uuid() == part_uuid_path.as_ref() {
            return (index + 1).try_into().context(format!(
                "Failed to convert index to u32 for partition '{}'",
                partition.path_by_uuid().display()
            ));
        }
    }

    bail!(
        "Failed to find the partition '{}' in disk '{}'",
        part_uuid_path.as_ref().display(),
        disk_path.as_ref().display()
    );
}

/// Gets the path of the root block device.
pub fn get_root_device_path() -> Result<PathBuf, TridentError> {
    let root_mount_path = if container::is_running_in_container()? {
        let host_root_path = container::get_host_root_path()?;
        debug!(
            "Running inside a container. Using root mount path '{}'",
            host_root_path.display()
        );
        host_root_path
    } else {
        debug!(
            "Not running inside a container. Using default root mount path '{}'",
            ROOT_MOUNT_POINT_PATH
        );
        Path::new(ROOT_MOUNT_POINT_PATH).to_path_buf()
    };

    let root_device_path =
        tabfile::get_device_path(Path::new(PROC_MOUNTINFO_PATH), &root_mount_path)
            .structured(InternalError::GetRootBlockDevicePath)?;

    Ok(root_device_path)
}

/// Unmounts all mount points associated with a given block device.
pub fn unmount_all_mount_points(block_device: impl AsRef<Path>) -> Result<(), Error> {
    trace!(
        "Unmounting all mount points for block device '{}'",
        block_device.as_ref().display()
    );

    // Get the mount points for the block device
    let mount_points = lsblk::get(block_device.as_ref())
        .with_context(|| {
            format!(
                "Failed to get mount points for block device '{}'",
                block_device.as_ref().display()
            )
        })?
        .mountpoints;

    // Attempt to unmount each mount point
    for mount_point in mount_points {
        // Unmount the mount point
        Dependency::Umount
            .cmd()
            .arg(&mount_point)
            .run_and_check()
            .with_context(|| {
                format!("Failed to unmount mount point '{}'", mount_point.display())
            })?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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

    use crate::{
        files,
        filesystems::{MkfsFileSystemType, MountFileSystemType},
        mkfs, mount,
        repart::{RepartEmptyMode, SystemdRepartInvoker},
        testutils::repart::{self, TEST_DISK_DEVICE_PATH},
        udevadm,
    };

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

    #[functional_test]
    fn test_partx_update_failure() {
        let disk_path = Path::new("/dev/does-not-exist");
        let err_out = partx_update(disk_path).unwrap_err();
        // Check contextual error message
        assert_eq!(
            err_out.to_string(),
            format!(
                "Failed to re-read partition table for disk '{}'",
                disk_path.display()
            )
        );
        // Check DependencyError in root cause
        assert!(err_out
            .root_cause()
            .to_string()
            .contains("Dependency 'partx' finished unsuccessfully"));
    }

    #[functional_test]
    fn test_get_root_device_path() {
        assert_eq!(
            get_root_device_path().unwrap().to_str().unwrap(),
            "/dev/sda2"
        );
    }

    #[functional_test]
    fn test_unmount_all_mount_points() {
        let parts = repart::generate_partition_definition_esp_root_generic();

        let parts = SystemdRepartInvoker::new(TEST_DISK_DEVICE_PATH, RepartEmptyMode::Force)
            .with_partition_entries(parts)
            .execute()
            .unwrap();

        udevadm::settle().unwrap();

        // Ensure no mount points exist before starting
        for part in parts.iter() {
            let blkdev = lsblk::get(&part.node).unwrap();
            assert_eq!(blkdev.mountpoints, Vec::<PathBuf>::new());
        }

        // Mount points to create per partition
        let mount_point_count = [1, 2, 3];
        assert_eq!(mount_point_count.len(), parts.len());

        let mut index = 0;
        // Create mount points for each partition
        for (part, mntp_count) in parts.iter().zip(mount_point_count.into_iter()) {
            // Create a filesystem on the partition
            mkfs::run(&part.node, MkfsFileSystemType::Ext4).unwrap();

            // Set the path for the mount point

            for _ in 0..mntp_count {
                let path = Path::new("/mnt").join("test").join(index.to_string());
                index += 1;

                files::create_dirs(&path).unwrap();

                mount::mount(&part.node, &path, MountFileSystemType::Ext4, &[]).unwrap();
            }

            // Check that the mount points were created
            let blkdev = lsblk::get(&part.node).unwrap();
            assert_eq!(blkdev.mountpoints.len(), mntp_count);
        }

        // Unmount all mount points
        for part in parts.iter() {
            unmount_all_mount_points(&part.node).unwrap();
        }

        // Check that all mount points were unmounted
        for part in parts.iter() {
            let blkdev = lsblk::get(&part.node).unwrap();
            assert_eq!(blkdev.mountpoints, Vec::<PathBuf>::new());
        }
    }
}
