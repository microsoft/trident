use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use anyhow::{Context, Error};
use log::{error, info};

use osutils::{filesystems::MountFileSystemType, lsof, mount, path::join_relative};
use trident_api::{
    config::InternalMountPoint,
    constants::{EXEC_ROOT_PATH, ROOT_MOUNT_POINT_PATH},
    error::{ManagementError, ReportError, TridentError},
    status::HostStatus,
};

use crate::modules::{self};

pub(super) fn unmount_new_root(
    mounts: Vec<PathBuf>,
    root_mount_path: &Path,
) -> Result<(), TridentError> {
    let mut mounts = mounts;
    mounts.reverse();
    let res = mounts.iter().try_for_each(|mount| {
        if *mount == join_relative(root_mount_path, EXEC_ROOT_PATH) {
            info!("Remounting '{}' as private", mount.display());
            mount::remount_rprivate(mount)
                .structured(ManagementError::UnmountNewroot { dir: mount.clone() })?;
        }

        info!("Unmounting '{}'", mount.display());
        mount::umount(mount, true)
            .structured(ManagementError::UnmountNewroot { dir: mount.clone() })
    });

    if res.is_err() {
        let opened_process_files = lsof::run(root_mount_path);
        // best effort, ignore failures here (such as missing external dependency)
        if let Ok(opened_process_files) = opened_process_files {
            if !opened_process_files.is_empty() {
                error!("Open files: {:?}", opened_process_files);
            }
        }
    }

    res
}

/// Returns an ordered map of mount points to their corresponding InternalMountPoint objects.
fn mount_points_map(host_status: &HostStatus) -> BTreeMap<&Path, &InternalMountPoint> {
    host_status
        .spec
        .storage
        .internal_mount_points
        .iter()
        .map(|mp| (&*mp.path, mp))
        .filter(|(path, _)| path.to_str() != Some("none"))
        .collect::<BTreeMap<_, _>>()
}

#[tracing::instrument(skip_all)]
pub(super) fn mount_new_root(
    host_status: &HostStatus,
    root_mount_path: &Path,
) -> Result<Vec<PathBuf>, TridentError> {
    info!("Mounting new root filesystems");

    mount_points_map(host_status)
        .iter()
        .map(|(path, mp)| {
            let target_path =
                root_mount_path.join(path.strip_prefix(ROOT_MOUNT_POINT_PATH).context(format!(
                    "Failed to strip prefix '{}' from '{}'",
                    ROOT_MOUNT_POINT_PATH,
                    path.display()
                ))?);

            info!(
                "Mounting block device '{}' to '{}'",
                mp.target_id,
                target_path.display()
            );

            mount::ensure_mount_directory(&target_path).context(format!(
                "Failed to prepare mount directory for block device '{}'",
                mp.target_id
            ))?;

            let device_path = modules::get_block_device(host_status, &mp.target_id, false)
                .context(format!(
                    "Failed to find block device path for id '{}'",
                    mp.target_id
                ))?
                .path;

            mount::mount(
                &device_path,
                &target_path,
                MountFileSystemType::from_api_type(mp.filesystem).context(format!(
                    "Filesystem type of block device '{}' is not valid for mounting: '{}'",
                    mp.target_id, mp.filesystem,
                ))?,
                &mp.options,
            )
            .context(format!(
                "Failed to mount block device '{}' with device path '{}' to '{}'",
                mp.target_id,
                device_path.display(),
                target_path.display()
            ))?;

            Ok(target_path)
        })
        .collect::<Result<Vec<PathBuf>, Error>>()
        .structured(ManagementError::MountNewroot)
}

#[cfg(test)]
mod test {
    use super::*;

    use std::path::PathBuf;
    use trident_api::config::{FileSystemType, HostConfiguration, InternalMountPoint, Storage};

    #[test]
    fn test_mount_point_ordering() {
        let host_status = HostStatus {
            spec: HostConfiguration {
                storage: Storage {
                    internal_mount_points: vec![
                        InternalMountPoint {
                            path: PathBuf::from("/mnt/boot/efi"),
                            target_id: "sda3".to_string(),
                            filesystem: FileSystemType::Vfat,
                            options: vec![],
                        },
                        InternalMountPoint {
                            path: PathBuf::from("/mnt"),
                            target_id: "sda1".to_string(),
                            filesystem: FileSystemType::Ext4,
                            options: vec![],
                        },
                        InternalMountPoint {
                            path: PathBuf::from("/a"),
                            target_id: "sda1".to_string(),
                            filesystem: FileSystemType::Ext4,
                            options: vec![],
                        },
                        InternalMountPoint {
                            path: PathBuf::from("/"),
                            target_id: "sda1".to_string(),
                            filesystem: FileSystemType::Ext4,
                            options: vec![],
                        },
                        InternalMountPoint {
                            path: PathBuf::from("/mnt/boot"),
                            target_id: "sda2".to_string(),
                            filesystem: FileSystemType::Ext4,
                            options: vec![],
                        },
                    ],
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };

        let paths = mount_points_map(&host_status)
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/"),
                PathBuf::from("/a"),
                PathBuf::from("/mnt"),
                PathBuf::from("/mnt/boot"),
                PathBuf::from("/mnt/boot/efi")
            ]
        );
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;
    use const_format::formatcp;
    use pytest_gen::functional_test;

    use std::{
        fs::{self, File},
        io::Read,
        path::{Path, PathBuf},
    };

    use maplit::btreemap;
    use tempfile::{NamedTempFile, TempDir};

    use osutils::{
        hashing_reader::HashingReader,
        image_streamer, mountpoint,
        repart::{RepartEmptyMode, SystemdRepartInvoker},
        testutils::repart::{
            self, CDROM_DEVICE_PATH, CDROM_MOUNT_PATH, PART1_SIZE, TEST_DISK_DEVICE_PATH,
        },
        udevadm,
    };
    use trident_api::{
        config::{self, Disk, FileSystemType, HostConfiguration, Partition, PartitionType},
        error::ErrorKind,
        status::{BlockDeviceInfo, Storage},
    };

    #[functional_test(feature = "helpers")]
    fn test_mount_and_umount() {
        // CDROM device to be mounted
        let device = Path::new(CDROM_DEVICE_PATH);
        // Mount point
        let mount_point = Path::new(CDROM_MOUNT_PATH);

        if mountpoint::check_is_mountpoint(mount_point).unwrap() {
            mount::umount(mount_point, false).unwrap();
        }

        // Create the mount point directory if it doesn't exist yet
        fs::create_dir_all(mount_point).unwrap();

        let host_status = HostStatus {
            spec: HostConfiguration {
                storage: config::Storage {
                    disks: vec![Disk {
                        id: "os".to_string(),
                        device: PathBuf::from("/dev/sr"),
                        partitions: vec![Partition {
                            id: "sr0".to_string(),
                            partition_type: PartitionType::Esp,
                            size: 0.into(),
                        }],
                        ..Default::default()
                    }],
                    internal_mount_points: vec![config::InternalMountPoint {
                        path: PathBuf::from("/"),
                        target_id: "sr0".to_string(),
                        filesystem: FileSystemType::Iso9660,
                        options: vec!["ro".into()],
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            storage: Storage {
                block_devices: btreemap! {
                    "os".into() => BlockDeviceInfo { path: PathBuf::from("/dev/sr"), size: 0 },
                    "sr0".into() => BlockDeviceInfo { path: PathBuf::from(CDROM_DEVICE_PATH), size: 0 }
                },
                ..Default::default()
            },
            ..Default::default()
        };

        let mounts = mount_new_root(&host_status, mount_point).unwrap();

        // If device is a file, fetch the name of loop device that was mounted at mount point;
        // otherwise, use the device path itself
        let loop_device = if device.is_file() {
            find_loop_device(device).unwrap()
        } else {
            device.to_string_lossy().to_string()
        };

        // Validate that the device has been successfully mounted
        assert!(
            is_device_mounted_at(&loop_device, mount_point),
            "Device not mounted at the expected mount point"
        );

        let root_mount_dir = tempfile::tempdir().unwrap();

        let host_status = HostStatus {
            spec: HostConfiguration {
                storage: config::Storage {
                    disks: vec![Disk {
                        id: "os".to_string(),
                        device: PathBuf::from(TEST_DISK_DEVICE_PATH),
                        partitions: vec![
                            Partition {
                                id: "esp".to_string(),
                                partition_type: PartitionType::Esp,
                                size: 0.into(),
                            },
                            Partition {
                                id: "root".to_string(),
                                partition_type: PartitionType::Root,
                                size: 0.into(),
                            },
                        ],
                        ..Default::default()
                    }],
                    internal_mount_points: vec![
                        config::InternalMountPoint {
                            path: PathBuf::from("/"),
                            target_id: "root".to_string(),
                            filesystem: FileSystemType::Ext4,
                            options: vec!["defaults".into()],
                        },
                        config::InternalMountPoint {
                            path: PathBuf::from("/boot/efi"),
                            target_id: "esp".to_string(),
                            filesystem: FileSystemType::Vfat,
                            options: vec!["umask=0077".into()],
                        },
                    ],
                    ..Default::default()
                },
                ..Default::default()
            },
            storage: Storage {
                block_devices: btreemap! {
                    "os".into() => BlockDeviceInfo { path: PathBuf::from(TEST_DISK_DEVICE_PATH), size: 0 },
                    "esp".into() => BlockDeviceInfo { path: PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}1")), size: 0 },
                    "root".into() => BlockDeviceInfo { path: PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2")), size: 0 }
                },
                ..Default::default()
            },
            ..Default::default()
        };

        // Partition test drive
        let partition_definition = repart::generate_partition_definition_esp_generic();
        let disk_bus_path = PathBuf::from(TEST_DISK_DEVICE_PATH);
        let repart = SystemdRepartInvoker::new(disk_bus_path, RepartEmptyMode::Force)
            .with_partition_entries(partition_definition.clone());
        let _ = repart.execute().unwrap();
        udevadm::settle().unwrap();

        // Image partitions on /dev/sdb
        let stream: Box<dyn Read> = Box::new(
            File::open("/data/esp.rawzst")
                .context("Failed to open esp image")
                .unwrap(),
        );
        image_streamer::stream_zstd(
            HashingReader::new(stream),
            Path::new(format!("{TEST_DISK_DEVICE_PATH}1").as_str()),
            Some(PART1_SIZE),
        )
        .unwrap();
        let stream: Box<dyn Read> = Box::new(
            File::open("/data/root.rawzst")
                .context("Failed to open root image")
                .unwrap(),
        );
        image_streamer::stream_zstd(
            HashingReader::new(stream),
            Path::new(format!("{TEST_DISK_DEVICE_PATH}2").as_str()),
            None,
        )
        .unwrap();

        // Test recursive mounting
        let mounts2 = mount_new_root(&host_status, root_mount_dir.path()).unwrap();
        assert!(root_mount_dir
            .path()
            .join("boot/efi/EFI/BOOT/bootx64.efi")
            .exists());
        unmount_new_root(mounts2, root_mount_dir.path()).unwrap();
        assert!(!root_mount_dir.path().join("boot").exists());

        // Test unmount_dir function
        unmount_new_root(mounts, mount_point).unwrap();

        // Validate that the device has been successfully unmounted
        assert!(
            !is_device_mounted_at(&loop_device, mount_point),
            "Device not unmounted"
        );
    }

    /// Checks if a device is mounted at a given mount point
    #[cfg(test)]
    fn is_device_mounted_at(device: impl AsRef<Path>, mount_point: impl AsRef<Path>) -> bool {
        let mounts = fs::read_to_string("/proc/mounts").expect("Failed to read /proc/mounts");
        for line in mounts.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2
                && parts[0] == device.as_ref().to_string_lossy()
                && parts[1] == mount_point.as_ref().to_string_lossy()
            {
                return true;
            }
        }
        false
    }

    /// Identifies the loop device associated with a given file
    #[cfg(test)]
    fn find_loop_device(file_path: &Path) -> Result<String, Error> {
        use std::process::Command;

        let output = Command::new("losetup")
            .arg("-j")
            .arg(file_path)
            .output()
            .context("Failed to execute losetup command")?;

        let output_str =
            String::from_utf8(output.stdout.clone()).context("Failed to parse losetup output")?;

        // Extract the loop device name from the losetup output
        output_str
            .lines()
            .next()
            .and_then(|line| line.split(':').next())
            .map(String::from)
            .ok_or_else(|| Error::msg("Failed to find loop device in losetup output"))
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_mount_failure() {
        let temp_mount_dir = TempDir::new().unwrap();

        // bad mount path
        let mut host_status = HostStatus {
            spec: HostConfiguration {
                storage: config::Storage {
                    disks: vec![Disk {
                        id: "os".to_string(),
                        device: PathBuf::from("/dev/sr"),
                        partitions: vec![Partition {
                            id: "sr0".to_string(),
                            partition_type: PartitionType::Esp,
                            size: 0.into(),
                        }],
                        ..Default::default()
                    }],
                    internal_mount_points: vec![config::InternalMountPoint {
                        path: PathBuf::from("foobar"),
                        target_id: "sr0".to_string(),
                        filesystem: FileSystemType::Iso9660,
                        options: vec!["bad-options".into()],
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            storage: Storage {
                block_devices: btreemap! {
                    "os".into() => BlockDeviceInfo { path: PathBuf::from("/dev/sr"), size: 0 },
                    "sr0".into() => BlockDeviceInfo { path: PathBuf::from(CDROM_DEVICE_PATH), size: 0 }
                },
                ..Default::default()
            },
            ..Default::default()
        };

        assert_eq!(
            mount_new_root(&host_status, temp_mount_dir.path())
                .unwrap_err()
                .kind(),
            &ErrorKind::Management(ManagementError::MountNewroot)
        );

        // bad root path
        let mut value = host_status.spec.storage.internal_mount_points.remove(0);
        value.path = PathBuf::from("/");
        host_status.spec.storage.internal_mount_points.push(value);
        let temp_file = NamedTempFile::new().unwrap();

        assert_eq!(
            mount_new_root(&host_status, temp_file.path())
                .unwrap_err()
                .kind(),
            &ErrorKind::Management(ManagementError::MountNewroot)
        );
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_umount_failure() {
        // Create a valid temporary directory
        let temp_mount_dir = TempDir::new().unwrap();

        // Test case 1: Attempt to unmount an existing directory that isn't mounted and assert that
        // it fails
        let umount_result_1 = unmount_new_root(
            vec![temp_mount_dir.path().to_owned()],
            temp_mount_dir.path(),
        );

        assert_eq!(
            umount_result_1.unwrap_err().kind(),
            &ErrorKind::Management(ManagementError::UnmountNewroot {
                dir: temp_mount_dir.path().to_owned()
            })
        );

        // Test case 2: Attempt to unmount a directory that does not exist
        let umount_result_2 = unmount_new_root(
            vec![PathBuf::from("/path/to/non/existent/directory")],
            temp_mount_dir.path(),
        );

        assert_eq!(
            umount_result_2.unwrap_err().kind(),
            &ErrorKind::Management(ManagementError::UnmountNewroot {
                dir: PathBuf::from("/path/to/non/existent/directory")
            })
        );
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_mount_with_populated_dir_failure() {
        // Mount point
        let temp_mount_dir = TempDir::new().unwrap();

        let host_status = HostStatus {
            spec: HostConfiguration {
                storage: config::Storage {
                    disks: vec![Disk {
                        id: "os".to_string(),
                        device: PathBuf::from("/dev/sr"),
                        partitions: vec![Partition {
                            id: "sr0".to_string(),
                            partition_type: PartitionType::Esp,
                            size: 0.into(),
                        }],
                        ..Default::default()
                    }],
                    internal_mount_points: vec![config::InternalMountPoint {
                        path: PathBuf::from("/"),
                        target_id: "sr0".to_string(),
                        filesystem: FileSystemType::Iso9660,
                        options: vec!["ro".into()],
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            storage: Storage {
                block_devices: btreemap! {
                    "os".into() => BlockDeviceInfo { path: PathBuf::from("/dev/sr"), size: 0 },
                    "sr0".into() => BlockDeviceInfo { path: PathBuf::from("/dev/sr0"), size: 0 }
                },
                ..Default::default()
            },
            ..Default::default()
        };

        // Create the mount point directory if it doesn't exist yet
        // Add a file to the mount point directory to simulate a populated directory
        let temp_mount_point_file = temp_mount_dir.path().join("temp_file");
        File::create(temp_mount_point_file).unwrap();

        // Attempt to mount the CDROM device to the mount point and assert that it fails
        let mount_result = mount_new_root(&host_status, temp_mount_dir.path())
            .expect_err("Expected mount_new_root to fail because of populated directory as path");

        assert_eq!(
            mount_result.kind(),
            &ErrorKind::Management(ManagementError::MountNewroot)
        );
    }
}
