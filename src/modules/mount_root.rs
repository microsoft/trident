use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Error};
use log::{error, info};
use osutils::{files, lsof, mount};
use trident_api::{
    constants::ROOT_MOUNT_POINT_PATH,
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

pub(super) fn mount_new_root(
    host_status: &HostStatus,
    root_mount_path: &Path,
) -> Result<Vec<PathBuf>, TridentError> {
    info!("Mounting new root filesystems");

    // Paths are ordered alphabetically, so we can mount them in the right order
    host_status
        .storage
        .mount_points
        .iter()
        .filter(|(path, _)| path.to_str() != Some("none"))
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

            if target_path.exists() {
                if !target_path.is_dir() {
                    bail!(
                        "Mount path '{}' for block device '{}' is not a directory",
                        target_path.display(),
                        mp.target_id
                    );
                }
            } else {
                // TODO handle read only filesystems, especially for the root
                // mount (will be done as part of the verity enablement)
                files::create_dirs(&target_path).context(format!(
                    "Failed to create mount path '{}' for block device '{}'",
                    target_path.display(),
                    mp.target_id
                ))?;
            }

            let device_path = modules::get_block_device(host_status, &mp.target_id, false)
                .context(format!(
                    "Failed to find block device path for id '{}'",
                    mp.target_id
                ))?
                .path;

            mount::mount(
                &device_path,
                &target_path,
                mp.filesystem.as_str(),
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
    use std::path::PathBuf;

    use maplit::btreemap;
    use trident_api::status::{MountPoint, Storage};

    use super::*;

    #[test]
    fn test_mount_point_ordering() {
        let host_status = HostStatus {
            storage: Storage {
                mount_points: vec![
                    (
                        PathBuf::from("/mnt/boot/efi"),
                        MountPoint {
                            target_id: "sda3".to_string(),
                            filesystem: "vfat".to_string(),
                            options: vec![],
                        },
                    ),
                    (
                        PathBuf::from("/mnt"),
                        MountPoint {
                            target_id: "sda1".to_string(),
                            filesystem: "ext4".to_string(),
                            options: vec![],
                        },
                    ),
                    (
                        PathBuf::from("/a"),
                        MountPoint {
                            target_id: "sda1".to_string(),
                            filesystem: "ext4".to_string(),
                            options: vec![],
                        },
                    ),
                    (
                        PathBuf::from("/"),
                        MountPoint {
                            target_id: "sda1".to_string(),
                            filesystem: "ext4".to_string(),
                            options: vec![],
                        },
                    ),
                    (
                        PathBuf::from("/mnt/boot"),
                        MountPoint {
                            target_id: "sda2".to_string(),
                            filesystem: "ext4".to_string(),
                            options: vec![],
                        },
                    ),
                ]
                .into_iter()
                .collect(),
                disks: btreemap! {},
                raid_arrays: btreemap! {},
                encrypted_volumes: btreemap! {},
                ab_update: None,
                root_device_path: None,
            },
            ..Default::default()
        };

        let paths = host_status
            .storage
            .mount_points
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
    use crate::modules::storage::image::stream_image;

    use super::*;
    use std::fs::File;
    use std::io::Read;
    use std::path::PathBuf;
    use std::{fs, path::Path};
    use tempfile::NamedTempFile;
    use tempfile::TempDir;

    use maplit::btreemap;
    use osutils::hashing_reader::HashingReader;
    use osutils::partition_types::DiscoverablePartitionType;
    use osutils::repart::{RepartMode, RepartPartitionEntry, SystemdRepartInvoker};
    use osutils::udevadm;
    use pytest_gen::functional_test;
    use trident_api::config::PartitionType;
    use trident_api::error::ErrorKind;
    use trident_api::status::Storage;
    use trident_api::status::{BlockDeviceContents, Disk};
    use trident_api::status::{MountPoint, Partition};
    use uuid::Uuid;

    const PART1_SIZE: u64 = 50 * 1024 * 1024; // 50 MiB
    const DISK_BUS_PATH: &str = "/dev/sdb";

    fn generate_partition_definition() -> Vec<RepartPartitionEntry> {
        vec![
            RepartPartitionEntry {
                partition_type: DiscoverablePartitionType::Esp,
                label: None,
                size_min_bytes: Some(PART1_SIZE),
                size_max_bytes: Some(PART1_SIZE),
            },
            RepartPartitionEntry {
                partition_type: DiscoverablePartitionType::LinuxGeneric,
                label: None,
                // When min==max==None, it's a grow partition
                size_min_bytes: None,
                size_max_bytes: None,
            },
        ]
    }

    #[functional_test(feature = "helpers")]
    fn test_mount_and_umount() {
        // CDROM device to be mounted
        let device = Path::new("/dev/sr0");
        // Mount point
        let mount_point = Path::new("/mnt/cdrom");

        // Create the mount point directory if it doesn't exist yet
        // fs::create_dir_all(mount_point).unwrap();

        let host_status = HostStatus {
            storage: Storage {
                mount_points: vec![(
                    PathBuf::from("/"),
                    MountPoint {
                        target_id: "sr0".to_string(),
                        filesystem: "iso9660".to_string(),
                        options: vec!["ro".into()],
                    },
                )]
                .into_iter()
                .collect(),
                disks: btreemap! {
                    "os".into() => Disk {
                        path: PathBuf::from("/dev/sr"),
                        uuid: Uuid::nil(),
                        capacity: 0,
                        contents: BlockDeviceContents::Unknown,
                        partitions: vec![
                            Partition {
                                id: "sr0".to_string(),
                                path: PathBuf::from("/dev/sr0"),
                                contents: BlockDeviceContents::Unknown,
                                start: 0,
                                end: 0,
                                ty: PartitionType::Esp,
                                uuid: Uuid::nil(),
                            },
                        ]
                    }

                },
                raid_arrays: Default::default(),
                encrypted_volumes: Default::default(),
                ab_update: None,
                root_device_path: None,
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
            storage: Storage {
                mount_points: vec![
                    (
                        PathBuf::from("/"),
                        MountPoint {
                            target_id: "root".to_string(),
                            filesystem: "ext4".to_string(),
                            options: vec!["defaults".into()],
                        },
                    ),
                    (
                        PathBuf::from("/boot/efi"),
                        MountPoint {
                            target_id: "esp".to_string(),
                            filesystem: "vfat".to_string(),
                            options: vec!["umask=0077".into()],
                        },
                    ),
                ]
                .into_iter()
                .collect(),
                disks: btreemap! {
                    "os".into() => Disk {
                        path: PathBuf::from("/dev/sdb"),
                        uuid: Uuid::nil(),
                        capacity: 0,
                        contents: BlockDeviceContents::Unknown,
                        partitions: vec![
                            Partition {
                                id: "esp".to_string(),
                                path: PathBuf::from("/dev/sdb1"),
                                contents: BlockDeviceContents::Unknown,
                                start: 0,
                                end: 0,
                                ty: PartitionType::Esp,
                                uuid: Uuid::nil(),
                            },
                            Partition {
                                id: "root".to_string(),
                                path: PathBuf::from("/dev/sdb2"),
                                contents: BlockDeviceContents::Unknown,
                                start: 0,
                                end: 0,
                                ty: PartitionType::Root,
                                uuid: Uuid::nil(),
                            },
                        ]
                    }

                },
                raid_arrays: Default::default(),
                encrypted_volumes: Default::default(),
                ab_update: None,
                root_device_path: None,
            },
            ..Default::default()
        };

        // Partition /dev/sdb
        let partition_definition = generate_partition_definition();
        let disk_bus_path = PathBuf::from(DISK_BUS_PATH);
        let repart = SystemdRepartInvoker::new(disk_bus_path, RepartMode::Force)
            .with_partition_entries(partition_definition.clone());
        let _ = repart.execute().unwrap();
        udevadm::settle().unwrap();

        // Image partitions on /dev/sdb
        let stream: Box<dyn Read> = Box::new(
            File::open(mount_point.join("data/esp.rawzst"))
                .context("Failed to open esp image")
                .unwrap(),
        );
        stream_image::stream_zstd_image_internal(
            HashingReader::new(stream),
            Path::new("/dev/sdb1"),
            Some(PART1_SIZE),
        )
        .unwrap();
        let stream: Box<dyn Read> = Box::new(
            File::open(mount_point.join("data/root.rawzst"))
                .context("Failed to open root image")
                .unwrap(),
        );
        stream_image::stream_zstd_image_internal(
            HashingReader::new(stream),
            Path::new("/dev/sdb2"),
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
            storage: Storage {
                mount_points: vec![(
                    PathBuf::from("foobar"),
                    MountPoint {
                        target_id: "sr0".to_string(),
                        filesystem: "iso9660".to_string(),
                        options: vec!["bad-option".into()],
                    },
                )]
                .into_iter()
                .collect(),
                disks: btreemap! {
                    "os".into() => Disk {
                        path: PathBuf::from("/dev/sr"),
                        uuid: Uuid::nil(),
                        capacity: 0,
                        contents: BlockDeviceContents::Unknown,
                        partitions: vec![
                            Partition {
                                id: "sr0".to_string(),
                                path: PathBuf::from("/dev/sr0"),
                                contents: BlockDeviceContents::Unknown,
                                start: 0,
                                end: 0,
                                ty: PartitionType::Esp,
                                uuid: Uuid::nil(),
                            },
                        ]
                    }

                },
                raid_arrays: Default::default(),
                encrypted_volumes: Default::default(),
                ab_update: None,
                root_device_path: None,
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
        let value = host_status
            .storage
            .mount_points
            .remove(&PathBuf::from("foobar"))
            .unwrap();
        host_status
            .storage
            .mount_points
            .insert(PathBuf::from("/"), value);
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
}
