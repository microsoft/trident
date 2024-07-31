use std::path::{Path, PathBuf};

use anyhow::{Context, Error};
use log::{debug, info};
use osutils::{filesystems::MkfsFileSystemType, mkfs, mkswap};
use rayon::prelude::*;
use trident_api::{
    config::{FileSystemSource, FileSystemType},
    status::{HostStatus, ServicingType},
    BlockDeviceId,
};

use crate::modules;

/// Creates clean filesystems on top of block devices that are not to be initialized with images,
/// i.e. have the file system source 'Create'. The function also re-formats any inactive/update A/B
/// volume with a clean FS, if the A/B volume pair is not requested to have an image.
#[tracing::instrument(skip_all)]
pub(super) fn create_filesystems(host_status: &mut HostStatus) -> Result<(), Error> {
    debug!("Creating filesystems on block devices");
    block_devices_needing_fs_creation(host_status)
        .par_iter()
        .map(|(block_device_id, device_path, filesystem)| {
            info!(
                "Creating '{}' filesystem on block device '{}'",
                filesystem, block_device_id
            );
            create_filesystem_on_block_device(device_path, *filesystem).context(format!(
                "Failed to create filesystem '{}' on block device '{}'",
                filesystem, block_device_id
            ))?;
            Ok(())
        })
        .collect()
}

/// Returns a list of tuples (block_device_id, device_path, filesystem) for block devices that need
/// to have clean filesystems created on them.
fn block_devices_needing_fs_creation(
    host_status: &HostStatus,
) -> Vec<(BlockDeviceId, PathBuf, FileSystemType)> {
    // Fetch the IDs of A/B volume pairs for filtering
    let ab_volume_pair_ids = host_status.spec.storage.get_ab_volume_pair_ids();

    // Iterate through all filesystems and filter out the ones that need to be created, composing
    // a list of block device IDs and filesystem types
    host_status
        .spec
        .storage
        .filesystems
        .iter()
        .filter_map(|fs| {
            // Filter to filesystems that need to be created
            match (&fs.source, host_status.servicing_type, &fs.device_id) {
                // If: the filesystem source is 'Create' AND device_id is present
                (FileSystemSource::Create, _, Some(device_id))

                // OR: the filesystem source is 'EspImage' AND servicing type
                // is CleanInstall AND device_id is present AND the ESP
                // partition is NOT an adopted partition
                | (
                    FileSystemSource::EspImage(_),
                    Some(ServicingType::CleanInstall),
                    Some(device_id),
                ) if !host_status.spec.storage.is_adopted_partition(device_id) => {
                    // Get the block device info for the device_id
                    modules::get_block_device(host_status, device_id, false)
                    .map(|bd_info| (device_id.clone(), bd_info, fs.fs_type))
                },

                // Otherwise, ignore the filesystem
                _ => None,
            }
        })
        .filter_map(|(device_id, bd_info, fs_type)| {
            // If the block device is an A/B volume pair and we're doing an A/B update, resolve
            // device_id to the device_id of the actual update volume
            if ab_volume_pair_ids.contains(&device_id)
                && host_status.servicing_type == Some(ServicingType::AbUpdate)
            {
                debug!(
                    "Servicing type is A/B update and A/B volume pair detected: {:?}",
                    device_id
                );
                modules::get_ab_volume_block_device_id(host_status, &device_id, false)
                    .map(|ab_volume_bdi| (ab_volume_bdi, bd_info.path, fs_type))
            // If the block device is NOT an A/B volume pair, only add it to block_devices if
            // a filesystem has not been previously created, i.e. we're doing a clean install
            } else if host_status.servicing_type == Some(ServicingType::CleanInstall) {
                debug!(
                    "Servicing type is clean install and a standalone volume detected: {:?}",
                    device_id
                );
                Some((device_id, bd_info.path, fs_type))
            } else {
                debug!(
                    "Volume is neither standalone nor A/B volume pair: {:?}",
                    device_id
                );
                None
            }
        })
        .collect()
}

/// Initialize a filesystem on the block device.
fn create_filesystem_on_block_device(
    device_path: &Path,
    filesystem: FileSystemType,
) -> Result<(), Error> {
    debug!(
        "Creating '{filesystem}' filesystem on block device {:?}",
        device_path
    );
    if filesystem == FileSystemType::Swap {
        mkswap::run(device_path).context("Failed to create swap space")
    } else {
        mkfs::run(
            device_path,
            MkfsFileSystemType::from_api_type(filesystem)
                .context("Swap should be handled separately")?,
        )
        .context("Failed to create filesystem")
    }
}

#[cfg(test)]
mod test {
    use std::path::PathBuf;

    use maplit::btreemap;
    use trident_api::{
        config::{
            self, AdoptedPartition, Disk, FileSystem, FileSystemSource, FileSystemType,
            HostConfiguration, Image, ImageFormat, ImageSha256, MountOptions, MountPoint,
            Partition, PartitionType, Storage as StorageConfig,
        },
        status::{AbVolumeSelection, BlockDeviceInfo, ServicingState, Storage},
    };

    use super::*;

    /// Validates that block_devices_needing_fs_creation () returns the correct list of block
    /// devices that need to have clean filesystems created on them.
    #[test]
    fn test_block_devices_needing_fs_creation() {
        let host_status_clean_install = HostStatus {
            servicing_type: Some(ServicingType::CleanInstall),
            servicing_state: ServicingState::Staging,
            spec: HostConfiguration {
                storage: StorageConfig {
                    disks: vec![Disk {
                        id: "os".to_owned(),
                        device: PathBuf::from("/dev/disk/by-bus/foobar"),
                        partitions: vec![
                            Partition {
                                id: "esp".to_owned(),
                                size: 100.into(),
                                partition_type: PartitionType::Esp,
                            },
                            Partition {
                                id: "root-a".to_owned(),
                                size: 100.into(),
                                partition_type: PartitionType::Root,
                            },
                            Partition {
                                id: "root-b".to_owned(),
                                size: 100.into(),
                                partition_type: PartitionType::Root,
                            },
                            Partition {
                                id: "trident".to_owned(),
                                size: 100.into(),
                                partition_type: PartitionType::LinuxGeneric,
                            },
                        ],
                        ..Default::default()
                    }],
                    filesystems: vec![
                        FileSystem {
                            device_id: Some("esp".into()),
                            fs_type: FileSystemType::Vfat,
                            source: FileSystemSource::EspImage(Image {
                                url: "http://example.com/esp_1.img".to_string(),
                                sha256: ImageSha256::Checksum("esp_sha256_1".to_string()),
                                format: ImageFormat::RawZst,
                            }),
                            mount_point: Some(MountPoint {
                                path: PathBuf::from("/esp"),
                                options: MountOptions::empty(),
                            }),
                        },
                        FileSystem {
                            device_id: Some("root".into()),
                            fs_type: FileSystemType::Ext4,
                            source: FileSystemSource::Image(Image {
                                url: "http://example.com/root_1.img".to_string(),
                                sha256: ImageSha256::Checksum("root_sha256_1".to_string()),
                                format: ImageFormat::RawZst,
                            }),
                            mount_point: Some(MountPoint {
                                path: PathBuf::from("/"),
                                options: MountOptions::empty(),
                            }),
                        },
                        FileSystem {
                            device_id: Some("trident".into()),
                            mount_point: Some(MountPoint {
                                path: "/trident".into(),
                                options: MountOptions::defaults(),
                            }),
                            fs_type: FileSystemType::Ext4,
                            source: FileSystemSource::Create,
                        },
                    ],
                    ab_update: Some(config::AbUpdate {
                        volume_pairs: vec![config::AbVolumePair {
                            id: "root".into(),
                            volume_a_id: "root-a".into(),
                            volume_b_id: "root-b".into(),
                        }],
                    }),
                    ..Default::default()
                },
                ..Default::default()
            },
            storage: Storage {
                block_devices: btreemap! {
                    "os".to_owned() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-bus/foobar"),
                        size: 34358672896,
                    },
                    "esp".to_owned() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-partlabel/osp1"),
                        size: 100,
                    },
                    "root-a".to_owned() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-partlabel/osp2"),
                        size: 100,
                    },
                    "root-b".to_owned() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-partlabel/osp3"),
                        size: 100,
                    },
                    "trident".into() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-partlabel/osp4"),
                        size: 100,
                    },
                },
                ..Default::default()
            },
            ..Default::default()
        };

        // Test case 1: On clean install, need to initialize the ESP partition and the standalone
        // volume 'trident'.
        let block_devices = block_devices_needing_fs_creation(&host_status_clean_install);
        assert_eq!(block_devices.len(), 2);
        assert!(block_devices.contains(&(
            "esp".into(),
            PathBuf::from("/dev/disk/by-partlabel/osp1"),
            FileSystemType::Vfat
        )));
        assert!(block_devices.contains(&(
            "trident".into(),
            PathBuf::from("/dev/disk/by-partlabel/osp4"),
            FileSystemType::Ext4
        )));

        // Test case 2: On A/B update, no need to initialize any FSs since all block devices either
        // have already had FSs created OR are being updated with an image.
        let mut host_status_ab_update = HostStatus {
            servicing_type: Some(ServicingType::AbUpdate),
            servicing_state: ServicingState::Staging,
            spec: HostConfiguration {
                storage: StorageConfig {
                    disks: vec![Disk {
                        id: "os".to_owned(),
                        device: PathBuf::from("/dev/disk/by-bus/foobar"),
                        partitions: vec![
                            Partition {
                                id: "esp".to_owned(),
                                size: 100.into(),
                                partition_type: PartitionType::Esp,
                            },
                            Partition {
                                id: "root-a".to_owned(),
                                size: 100.into(),
                                partition_type: PartitionType::Root,
                            },
                            Partition {
                                id: "root-b".to_owned(),
                                size: 100.into(),
                                partition_type: PartitionType::Root,
                            },
                            Partition {
                                id: "trident".to_owned(),
                                size: 100.into(),
                                partition_type: PartitionType::LinuxGeneric,
                            },
                        ],
                        ..Default::default()
                    }],
                    filesystems: vec![
                        FileSystem {
                            device_id: Some("esp".into()),
                            fs_type: FileSystemType::Vfat,
                            source: FileSystemSource::EspImage(Image {
                                url: "http://example.com/esp_2.img".to_string(),
                                sha256: ImageSha256::Checksum("esp_sha256_2".to_string()),
                                format: ImageFormat::RawZst,
                            }),
                            mount_point: Some(MountPoint {
                                path: PathBuf::from("/esp"),
                                options: MountOptions::empty(),
                            }),
                        },
                        FileSystem {
                            device_id: Some("root".into()),
                            fs_type: FileSystemType::Ext4,
                            source: FileSystemSource::Image(Image {
                                url: "http://example.com/root_2.img".to_string(),
                                sha256: ImageSha256::Checksum("root_sha256_2".to_string()),
                                format: ImageFormat::RawZst,
                            }),
                            mount_point: Some(MountPoint {
                                path: PathBuf::from("/"),
                                options: MountOptions::empty(),
                            }),
                        },
                        FileSystem {
                            device_id: Some("trident".into()),
                            mount_point: Some(MountPoint {
                                path: "/trident".into(),
                                options: MountOptions::defaults(),
                            }),
                            fs_type: FileSystemType::Ext4,
                            source: FileSystemSource::Create,
                        },
                    ],
                    ab_update: Some(config::AbUpdate {
                        volume_pairs: vec![config::AbVolumePair {
                            id: "root".into(),
                            volume_a_id: "root-a".into(),
                            volume_b_id: "root-b".into(),
                        }],
                    }),
                    ..Default::default()
                },
                ..Default::default()
            },
            storage: Storage {
                block_devices: btreemap! {
                    "os".to_owned() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-bus/foobar"),
                        size: 34358672896,
                    },
                    "esp".to_owned() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-partlabel/osp1"),
                        size: 100,
                    },
                    "root-a".to_owned() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-partlabel/osp2"),
                        size: 100,
                    },
                    "root-b".to_owned() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-partlabel/osp3"),
                        size: 100,
                    },
                    "trident".into() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-partlabel/osp4"),
                        size: 100,
                    },
                },
                ab_active_volume: Some(AbVolumeSelection::VolumeA),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(block_devices_needing_fs_creation(&host_status_ab_update).is_empty());

        // Test case 3: If the A/B volume pair now does not have an image requested for it, we need
        // to initialize the filesystem on the A/B volume pair.
        // Update the filesystem for 'root'
        host_status_ab_update.spec.storage.filesystems[1] = FileSystem {
            device_id: Some("root".into()),
            fs_type: FileSystemType::Ext4,
            source: FileSystemSource::Create,
            mount_point: Some(MountPoint {
                path: PathBuf::from("/"),
                options: MountOptions::empty(),
            }),
        };
        let block_devices = block_devices_needing_fs_creation(&host_status_ab_update);
        assert_eq!(block_devices.len(), 1);
        assert!(block_devices.contains(&(
            "root-b".into(),
            PathBuf::from("/dev/disk/by-partlabel/osp3"),
            FileSystemType::Ext4
        )));
    }

    /// Test that block_devices_needing_fs_creation() does not return any block
    /// devices that are adopted ESP partitions.
    #[test]
    fn test_block_devices_needing_fs_creation_adopted_esp() {
        let host_status = HostStatus {
            servicing_type: Some(ServicingType::AbUpdate),
            servicing_state: ServicingState::Staging,
            spec: HostConfiguration {
                storage: StorageConfig {
                    disks: vec![Disk {
                        id: "os".to_owned(),
                        device: PathBuf::from("/dev/disk/by-bus/foobar"),
                        partitions: vec![Partition {
                            id: "root-b".to_owned(),
                            size: 100.into(),
                            partition_type: PartitionType::Root,
                        }],
                        adopted_partitions: vec![
                            AdoptedPartition {
                                id: "esp".to_owned(),
                                match_label: Some("esp".to_owned()),
                                match_uuid: None,
                            },
                            AdoptedPartition {
                                id: "root-a".to_owned(),
                                match_label: Some("root-a".to_owned()),
                                match_uuid: None,
                            },
                        ],
                        ..Default::default()
                    }],
                    filesystems: vec![
                        FileSystem {
                            device_id: Some("esp".into()),
                            fs_type: FileSystemType::Vfat,
                            source: FileSystemSource::EspImage(Image {
                                url: "http://example.com/esp_2.img".to_string(),
                                sha256: ImageSha256::Checksum("esp_sha256_2".to_string()),
                                format: ImageFormat::RawZst,
                            }),
                            mount_point: Some(MountPoint {
                                path: PathBuf::from("/esp"),
                                options: MountOptions::empty(),
                            }),
                        },
                        FileSystem {
                            device_id: Some("root-b".into()),
                            fs_type: FileSystemType::Ext4,
                            source: FileSystemSource::Image(Image {
                                url: "http://example.com/root_2.img".to_string(),
                                sha256: ImageSha256::Checksum("root_sha256_2".to_string()),
                                format: ImageFormat::RawZst,
                            }),
                            mount_point: Some(MountPoint {
                                path: PathBuf::from("/"),
                                options: MountOptions::empty(),
                            }),
                        },
                    ],
                    ..Default::default()
                },
                ..Default::default()
            },
            storage: Storage {
                block_devices: btreemap! {
                    "os".to_owned() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-bus/foobar"),
                        size: 34358672896,
                    },
                    "esp".to_owned() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-partlabel/osp1"),
                        size: 100,
                    },
                    "root-a".to_owned() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-partlabel/osp2"),
                        size: 100,
                    },
                    "root-b".to_owned() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-partlabel/osp3"),
                        size: 100,
                    },
                },
                ab_active_volume: Some(AbVolumeSelection::VolumeA),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            block_devices_needing_fs_creation(&host_status),
            vec![],
            "No filesystems should be created for adopted partitions"
        );
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;
    use pytest_gen::functional_test;

    use std::process::Command;

    use const_format::formatcp;

    use osutils::{
        exe::RunAndCheck,
        filesystems::MountFileSystemType,
        lsblk, mount,
        testutils::repart::{self, TEST_DISK_DEVICE_PATH},
    };

    #[functional_test(feature = "helpers")]
    /// Validates that initialize_block_device() correctly initializes a block device by formatting it
    /// to the specified filesystem.
    fn test_create_filesystem_on_block_device() {
        // Test case 1: Running initialize_block_device() on a valid block device to format as ext4.
        // First, zero out the metadata of the volume. Use /dev/sdb since cannot rely on
        // /dev/sdb2 being present.
        repart::clear_disk(Path::new(TEST_DISK_DEVICE_PATH)).unwrap();

        // Run initialize_block_device() to format to ext4 filesystem
        create_filesystem_on_block_device(Path::new(TEST_DISK_DEVICE_PATH), FileSystemType::Ext4)
            .unwrap();

        // Confirm that /dev/sdb has been reformatted to ext4
        let block_device = lsblk::run(Path::new(TEST_DISK_DEVICE_PATH)).unwrap();

        // Find the current FS on /dev/sdb
        assert_eq!(
            block_device.fstype.as_ref().unwrap(),
            "ext4",
            "Filesystem type on /dev/sdb is not ext4"
        );

        // Create /mnt/sdb if does not exist and confirm that /dev/sdb can be mounted
        Command::new("mkdir")
            .arg("-p")
            .arg("/mnt/sdb")
            .output_and_check()
            .unwrap();

        mount::mount(
            Path::new(TEST_DISK_DEVICE_PATH),
            Path::new("/mnt/sdb"),
            MountFileSystemType::Ext4,
            &["defaults".into()],
        )
        .unwrap();

        // Unmount /dev/sdb
        mount::umount(Path::new("/mnt/sdb"), false).unwrap();
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_create_filesystem_on_block_device_negative() {
        // Just zero-out the metadata so this is a fast operation.
        repart::clear_disk(Path::new(TEST_DISK_DEVICE_PATH)).unwrap();

        let result = create_filesystem_on_block_device(
            Path::new(formatcp!("{TEST_DISK_DEVICE_PATH}2")),
            FileSystemType::Ext4,
        );

        assert_eq!(
                result.unwrap_err().root_cause().to_string(),
                "Process output:\nstderr:\nmke2fs 1.46.5 (30-Dec-2021)\nThe file /dev/sdb2 does not exist and no size was specified.\n\n",
                "Failed to initialize block device that does not exist"
            );
    }
}
