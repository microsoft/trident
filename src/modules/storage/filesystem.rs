use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use anyhow::{Context, Error};
use log::{debug, info};
use osutils::{filesystems::MkfsFileSystemType, mkfs, mkswap};
use rayon::prelude::*;
use trident_api::{
    config::FileSystemType,
    status::{BlockDeviceContents, HostStatus, ReconcileState, UpdateKind},
    BlockDeviceId,
};

use crate::modules;
use crate::modules::storage;

use super::image;

/// Determines which block devices will not be initialized using images and
/// formats them with a desired filesystem. The logic picks any uninitialized
/// block devices with assigned mount points and for A/B update, also devices
/// the inactive block devices, that are part of A/B volume pairs, to make sure
/// they are reinitialized when needed.
pub(super) fn create_filesystems(host_status: &mut HostStatus) -> Result<(), Error> {
    debug!("Creating filesystems on block devices");
    get_block_devices_to_initialize(host_status)
        .par_iter()
        .map(|(block_device_id, device_path, filesystem)| {
            info!(
                "Creating '{}' filesystem on block device '{:?}'",
                filesystem, block_device_id
            );
            create_filesystem_on_block_device(device_path, *filesystem).context(format!(
                "Failed to create filesystem '{}' on block device '{}'",
                filesystem, block_device_id
            ))?;
            Ok(block_device_id)
        })
        .collect::<Vec<_>>()
        .into_iter()
        .try_for_each(|block_device_id| match block_device_id {
            Err(e) => Err(e),
            Ok(block_device_id) => storage::set_host_status_block_device_contents(
                host_status,
                block_device_id,
                BlockDeviceContents::Initialized,
            )
            .context(format!(
                "Failed to set block device contents for block device '{}'",
                block_device_id,
            )),
        })
}

/// Determines which block devices will not be initialized using images or needs
/// to be reinitialized for A/B update.
///
/// Returns a tuple of the block device id, info to update and filesystem to
/// deploy on it.
fn get_block_devices_to_initialize(
    host_status: &HostStatus,
) -> Vec<(BlockDeviceId, PathBuf, FileSystemType)> {
    // Fetch the list of block devices initialized by images
    let requested_image_block_device_ids: HashSet<&BlockDeviceId> = host_status
        .spec
        .storage
        .images
        .iter()
        .map(|image| &image.target_id)
        .collect();

    // Filter mount points out if they point to block devices that are
    // initialized by images
    let candidates = host_status
        .spec
        .storage
        .mount_points
        .iter()
        .filter(|mount_point| {
            // Skip mount points that are initialized by images
            !requested_image_block_device_ids.contains(&mount_point.target_id)
            // If this is Clean Install, we need to special case ESP and
            // initialize it here
                || (host_status.reconcile_state == ReconcileState::CleanInstall
                    && image::is_esp(&host_status.spec, &mount_point.target_id))
        });

    // Select mount points that have been uninitialized or in case of A/B
    // update, need to be cleaned (in case of B->A update, we dont want to
    // mount data from the previous iteration of A)
    let selected = candidates
        .filter_map(|mount_point| {
            modules::get_block_device(host_status, &mount_point.target_id, false)
                .map(|bdi| (mount_point, bdi))
        })
        .filter_map(|(mount_point, block_device_info)| {
            let ab_volume_pair = host_status
                .spec
                .storage
                .ab_update
                .as_ref()
                .map(|ab_update| {
                    ab_update
                        .volume_pairs
                        .iter()
                        .any(|p| p.id == mount_point.target_id)
                })
                .unwrap_or(false);

            if matches!(
                block_device_info.contents,
                BlockDeviceContents::Unknown | BlockDeviceContents::Zeroed
            ) {
                // If this has never been initialized, do it now.
                return Some((
                    mount_point.target_id.clone(),
                    block_device_info.path,
                    mount_point.filesystem,
                    ab_volume_pair,
                ));
            }

            if host_status.reconcile_state == ReconcileState::UpdateInProgress(UpdateKind::AbUpdate)
                && ab_volume_pair
            {
                // If this is an A/B volume pair, reinitialize it
                return Some((
                    mount_point.target_id.clone(),
                    block_device_info.path,
                    mount_point.filesystem,
                    ab_volume_pair,
                ));
            }

            // In all other cases, we cannot touch it, as it could lead to data loss
            None
        });

    // Resolve A/B update volume pairs to the underlying block devices
    let resolved = selected.filter_map(
        |(block_device_id, device_path, filesystem, ab_volume_pair)| {
            if ab_volume_pair {
                // If this is an A/B volume pair, point to the right
                // underlying block device to be reinitialized
                // Ok to ignore None from get_ab_volume_block_device_id,
                // as API check enforces consistency
                modules::get_ab_volume_block_device_id(host_status, &block_device_id, false)
                    .map(|child_block_device_id| (child_block_device_id, device_path, filesystem))
            } else {
                Some((block_device_id, device_path, filesystem))
            }
        },
    );

    resolved.collect()
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
            self, AbUpdate, AbVolumePair, Disk, HostConfiguration, Image, ImageFormat, ImageSha256,
            MountPoint, Partition, PartitionSize, PartitionType, Storage as StorageConfig,
        },
        constants::ROOT_MOUNT_POINT_PATH,
        status::{AbVolumeSelection, BlockDeviceInfo, Storage},
    };

    use super::*;

    /// Validates that get_block_devices_to_initialize() returns the correct
    /// list of block devices that need to be initialized.
    #[test]
    fn test_get_block_devices_to_initialize() {
        // Setup HostStatus where image is requested for volume pair with id root
        let host_status_golden = HostStatus {
            reconcile_state: ReconcileState::CleanInstall,
            spec: HostConfiguration {
                storage: StorageConfig {
                    disks: vec![Disk {
                        id: "os".to_owned(),
                        device: PathBuf::from("/dev/disk/by-bus/foobar"),
                        partitions: vec![
                            Partition {
                                id: "esp".to_owned(),
                                size: PartitionSize::Fixed(100),
                                partition_type: PartitionType::Esp,
                            },
                            Partition {
                                id: "root-a".to_owned(),
                                size: PartitionSize::Fixed(1000),
                                partition_type: PartitionType::Root,
                            },
                            Partition {
                                id: "root-b".to_owned(),
                                size: PartitionSize::Fixed(10000),
                                partition_type: PartitionType::Root,
                            },
                        ],
                        ..Default::default()
                    }],
                    mount_points: vec![
                        MountPoint {
                            path: PathBuf::from("/boot/efi"),
                            target_id: "esp".to_string(),
                            filesystem: FileSystemType::Vfat,
                            options: vec![],
                        },
                        MountPoint {
                            path: PathBuf::from(ROOT_MOUNT_POINT_PATH),
                            target_id: "root".to_string(),
                            filesystem: FileSystemType::Ext4,
                            options: vec![],
                        },
                    ],
                    images: vec![Image {
                        url: "http://example.com/root_3.img".to_string(),
                        target_id: "root".to_string(),
                        format: ImageFormat::RawZst,
                        sha256: ImageSha256::Checksum("root_sha256_3".to_string()),
                    }],
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
                        size: 0,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "esp".to_owned() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-partlabel/osp1"),
                        size: 100,
                        contents: BlockDeviceContents::Image {
                            url: "http://example.com/esp_1.img".to_string(),
                            sha256: "esp_sha256_1".to_string(),
                            length: 100,
                        },
                    },
                    "root-a".to_owned() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-partlabel/osp2"),
                        size: 1000,
                        contents: BlockDeviceContents::Image {
                            url: "http://example.com/root_1.img".to_string(),
                            sha256: "root_sha256_1".to_string(),
                            length: 100,
                        },
                    },
                    "root-b".to_owned() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-partlabel/osp3"),
                        size: 10000,
                        contents: BlockDeviceContents::Image {
                            url: "http://example.com/root_2.img".to_string(),
                            sha256: "root_sha256_2".to_string(),
                            length: 100,
                        },
                    },

                },
                ..Default::default()
            },
            ..Default::default()
        };

        // Test case 1: Running get_block_devices_to_initialize() with host's status set to CleanInstall
        // should return an empty vector, as all block devices have been already initialized
        assert_eq!(
            get_block_devices_to_initialize(&host_status_golden),
            Vec::<(BlockDeviceId, PathBuf, FileSystemType)>::new(),
            "Failed to determine that no block devices should be initialized on CleanInstall"
        );

        // Test case 2: Running get_block_devices_to_initialize() with host's status set to CleanInstall
        // and some devices uninitialized or zeroed, should not return empty
        // vector
        let mut host_status = host_status_golden.clone();
        host_status
            .storage
            .block_devices
            .get_mut("esp")
            .unwrap()
            .contents = BlockDeviceContents::Unknown;
        // Only one should be returned, because the A/B volume pair is
        // initialized by an image
        assert_eq!(
            get_block_devices_to_initialize(&host_status),
            vec![(
                "esp".to_owned(),
                PathBuf::from("/dev/disk/by-partlabel/osp1"),
                FileSystemType::Vfat,
            )],
            "Failed to determine which block devices should be initialized on CleanInstall"
        );

        // Test case 2b: Running get_block_devices_to_initialize() with host's status set to CleanInstall
        // and some devices uninitialized or zeroed, should not return empty
        // vector
        host_status
            .storage
            .block_devices
            .get_mut("root-a")
            .unwrap()
            .contents = BlockDeviceContents::Zeroed;
        host_status
            .storage
            .block_devices
            .get_mut("root-b")
            .unwrap()
            .contents = BlockDeviceContents::Zeroed;
        // Only one should be returned, because the A/B volume pair is
        // initialized by an image
        assert_eq!(
            get_block_devices_to_initialize(&host_status),
            vec![(
                "esp".to_owned(),
                PathBuf::from("/dev/disk/by-partlabel/osp1"),
                FileSystemType::Vfat,
            )],
            "Failed to determine which block devices should be initialized on CleanInstall"
        );

        // Test case 3: Running get_block_devices_to_initialize() with host's status set to CleanInstall
        // and some devices uninitialized or zeroed, should not return empty
        // vector
        host_status.spec = host_status_golden.spec.clone();
        host_status.spec.storage.images.clear();
        assert_eq!(
            get_block_devices_to_initialize(&host_status),
            vec![
                (
                    "esp".to_owned(),
                    PathBuf::from("/dev/disk/by-partlabel/osp1"),
                    FileSystemType::Vfat,
                ),
                (
                    "root-a".to_owned(),
                    PathBuf::from("/dev/disk/by-partlabel/osp2"),
                    FileSystemType::Ext4,
                )
            ],
            "Failed to determine which block devices should be initialized on CleanInstall"
        );

        // Test case 4: Set host's status to UpdateInProgress(AbUpdate) and set active volume to A.
        // Running get_block_devices_to_initialize() when there is an image requested for A/B volume pair with
        // id root should return an empty vector
        let mut host_status = host_status_golden.clone();
        host_status.reconcile_state = ReconcileState::UpdateInProgress(UpdateKind::AbUpdate);
        host_status.spec.storage.ab_update = Some(AbUpdate {
            volume_pairs: vec![AbVolumePair {
                id: "root".to_string(),
                volume_a_id: "root-a".to_string(),
                volume_b_id: "root-b".to_string(),
            }],
        });
        host_status.storage.ab_active_volume = Some(AbVolumeSelection::VolumeA);

        assert_eq!(
                get_block_devices_to_initialize(&host_status),
                Vec::<(BlockDeviceId, PathBuf, FileSystemType)>::new(),
                "Failed to determine that no volumes should be reinitialized when images for all A/B volume pairs are requested"
            );

        // Test case 5: Remove image for target_id root from HostStatus. Running
        // get_volumes_to_reinitialize() should now return a vector containing the target_id of the volume
        // pair with id root
        host_status.spec.storage.images = vec![];

        let expected_path_rootb = PathBuf::from("/dev/disk/by-partlabel/osp3");

        // Vector is expected to contain "root-b" since A is active volume
        let expected_volume_rootb = vec![(
            "root-b".to_owned(),
            expected_path_rootb.clone(),
            FileSystemType::Ext4,
        )];

        assert_eq!(
                get_block_devices_to_initialize(&host_status),
                expected_volume_rootb,
                "Failed to determine that volume root-b should be reinitialized when image for A/B volume pair root is missing and active volume is A"
            );

        // Test case 4: Set active volume to B. Now, vector is expected to contain "root-a"
        host_status.storage.ab_active_volume = Some(AbVolumeSelection::VolumeB);

        let expected_path_roota = PathBuf::from("/dev/disk/by-partlabel/osp2");

        let expected_volume_roota = vec![(
            "root-a".to_owned(),
            expected_path_roota.clone(),
            FileSystemType::Ext4,
        )];

        assert_eq!(
                get_block_devices_to_initialize(&host_status),
                expected_volume_roota,
                "Failed to determine that volume root-1 should be reinitialized when image for A/B volume pair root is missing and active volume is B"
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
