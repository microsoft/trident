use std::path::{Path, PathBuf};

use anyhow::{Context, Error};
use log::{debug, info, trace};
use rayon::prelude::*;

use osutils::{filesystems::MkfsFileSystemType, mkfs};
use trident_api::{config::FileSystemType, status::ServicingType, BlockDeviceId};

use crate::engine::{context::filesystem::FileSystemData, EngineContext};

/// Creates clean filesystems on top of block devices that are not to be initialized with images,
/// i.e. have the file system source 'Create'. The function also re-formats any inactive/update A/B
/// volume with a clean FS, if the A/B volume pair is not requested to have an image.
#[tracing::instrument(name = "filesystems_creation", skip_all)]
pub(super) fn create_filesystems(ctx: &EngineContext) -> Result<(), Error> {
    debug!("Creating filesystems on block devices");
    block_devices_needing_fs_creation(ctx)
        .context("Failed to obtain list of block devices needing filesystem creation.")?
        .par_iter()
        .map(|(block_device_id, device_path, filesystem)| {
            info!("Initializing '{block_device_id}': creating filesystem of type '{filesystem}'");
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
    ctx: &EngineContext,
) -> Result<Vec<(BlockDeviceId, PathBuf, FileSystemType)>, Error> {
    debug!("Determining block devices needing filesystem creation");
    // Fetch the IDs of A/B volume pairs for filtering.
    let ab_volume_pair_ids = ctx.spec.storage.get_ab_volume_pair_ids();

    // Iterate through all filesystems and filter out the ones that need to be created, composing
    // a list of block device IDs and filesystem types.
    let mut block_devices = Vec::new();

    for fs in &ctx.filesystems {
        // Filter to the filesystems matching any of the specified criteria:
        let device_id = match &fs {
            // The filesystem source is 'New'.
            FileSystemData::New(nfs) => &nfs.device_id,

            // The filesystem source is `Image` AND servicing type is
            // CleanInstall AND the mount point is the ESP location.
            FileSystemData::Image(ifs)
                if ctx.servicing_type == ServicingType::CleanInstall && fs.is_esp() =>
            {
                &ifs.device_id
            }

            // Otherwise, ignore and skip the filesystem.
            // Filter out all 'Adopted' filesystems, tmpfs and overlay
            // filesystems, and any non-ESP filesystems on an image.
            _ => continue,
        };

        // Get the block device info for the device_id
        let bd_path = ctx.get_block_device_path(device_id).with_context(|| {
            format!("Block device path not found for device ID: {:?}", device_id)
        })?;

        let effective_device_id = if ab_volume_pair_ids.contains(device_id)
            && ctx.servicing_type == ServicingType::AbUpdate
        {
            // If the block device is an A/B volume pair and we're doing an A/B
            // update, resolve device_id to the device_id of the actual update
            // volume.
            trace!(
                "Servicing type is A/B update and A/B volume pair detected: {:?}",
                device_id
            );

            ctx.get_ab_volume_block_device_id(device_id)
                .with_context(|| {
                    format!(
                        "Failed to resolve A/B volume pair ID to update volume ID: {:?}",
                        device_id
                    )
                })?
        } else if ctx.servicing_type == ServicingType::CleanInstall {
            // If the block device is NOT an A/B volume pair, only add it to
            // block_devices if a filesystem has not been previously created,
            // i.e. we're doing a clean install.
            trace!(
                "Servicing type is clean install and a standalone volume detected: {:?}",
                device_id
            );
            device_id
        } else {
            trace!(
                "Volume is neither standalone nor A/B volume pair: {:?}",
                device_id
            );
            continue;
        };

        block_devices.push((effective_device_id.clone(), bd_path, fs.fs_type()));
    }

    debug!(
        "Found {} block device{} needing filesystem creation",
        block_devices.len(),
        if block_devices.len() == 1 { "" } else { "s" }
    );

    Ok(block_devices)
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

    mkfs::run(
        device_path,
        MkfsFileSystemType::from_api_type(filesystem)
            .context("Swap should be handled separately")?,
    )
    .context("Failed to create filesystem")
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    use maplit::btreemap;

    use trident_api::{
        config::{
            self, AdoptedPartition, Disk, FileSystem, FileSystemSource, FileSystemType,
            HostConfiguration, MountOptions, MountPoint, Partition, PartitionType,
            Storage as StorageConfig,
        },
        status::AbVolumeSelection,
    };

    /// Validates that block_devices_needing_fs_creation () returns the correct list of block
    /// devices that need to have clean filesystems created on them.
    #[test]
    fn test_block_devices_needing_fs_creation() {
        let mut ctx_clean_install = EngineContext {
            servicing_type: ServicingType::CleanInstall,
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
                            source: FileSystemSource::Image,
                            mount_point: Some(MountPoint {
                                path: PathBuf::from("/boot/efi"),
                                options: MountOptions::empty(),
                            }),
                        },
                        FileSystem {
                            device_id: Some("root".into()),
                            fs_type: FileSystemType::Ext4,
                            source: FileSystemSource::Image,
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
                            source: FileSystemSource::New,
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
            partition_paths: btreemap! {
                "os".to_owned() => PathBuf::from("/dev/disk/by-bus/foobar"),
                "esp".to_owned() => PathBuf::from("/dev/disk/by-partlabel/osp1"),
                "root-a".to_owned() => PathBuf::from("/dev/disk/by-partlabel/osp2"),
                "root-b".to_owned() => PathBuf::from("/dev/disk/by-partlabel/osp3"),
                "trident".into() => PathBuf::from("/dev/disk/by-partlabel/osp4"),
            },
            ..Default::default()
        };
        ctx_clean_install.populate_filesystems().unwrap();

        // Test case 1: On clean install, need to initialize the ESP partition and the standalone
        // volume 'trident'.
        let block_devices = block_devices_needing_fs_creation(&ctx_clean_install).unwrap();
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
        let mut ctx_ab_update = EngineContext {
            servicing_type: ServicingType::AbUpdate,
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
                            source: FileSystemSource::Image,
                            mount_point: Some(MountPoint {
                                path: PathBuf::from("/esp"),
                                options: MountOptions::empty(),
                            }),
                        },
                        FileSystem {
                            device_id: Some("root".into()),
                            fs_type: FileSystemType::Ext4,
                            source: FileSystemSource::Image,
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
                            source: FileSystemSource::New,
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
            partition_paths: btreemap! {
                "os".to_owned() => PathBuf::from("/dev/disk/by-bus/foobar"),
                "esp".to_owned() => PathBuf::from("/dev/disk/by-partlabel/osp1"),
                "root-a".to_owned() => PathBuf::from("/dev/disk/by-partlabel/osp2"),
                "root-b".to_owned() => PathBuf::from("/dev/disk/by-partlabel/osp3"),
                "trident".into() => PathBuf::from("/dev/disk/by-partlabel/osp4"),
            },
            ab_active_volume: Some(AbVolumeSelection::VolumeA),
            ..Default::default()
        };
        ctx_ab_update.populate_filesystems().unwrap();
        assert!(block_devices_needing_fs_creation(&ctx_ab_update)
            .unwrap()
            .is_empty());

        // Test case 3: If the A/B volume pair now does not have an image requested for it, we need
        // to initialize the filesystem on the A/B volume pair.
        // Update the filesystem for 'root'
        ctx_ab_update.spec.storage.filesystems[1] = FileSystem {
            device_id: Some("root".into()),
            fs_type: FileSystemType::Ext4,
            source: FileSystemSource::New,
            mount_point: Some(MountPoint {
                path: PathBuf::from("/"),
                options: MountOptions::empty(),
            }),
        };
        ctx_ab_update.populate_filesystems().unwrap();
        let block_devices = block_devices_needing_fs_creation(&ctx_ab_update).unwrap();
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
        let ctx = EngineContext {
            servicing_type: ServicingType::AbUpdate,
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
                            source: FileSystemSource::Image,
                            mount_point: Some(MountPoint {
                                path: PathBuf::from("/boot/efi"),
                                options: MountOptions::empty(),
                            }),
                        },
                        FileSystem {
                            device_id: Some("root-b".into()),
                            fs_type: FileSystemType::Ext4,
                            source: FileSystemSource::Image,
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
            partition_paths: btreemap! {
                "os".to_owned() => PathBuf::from("/dev/disk/by-bus/foobar"),
                "esp".to_owned() => PathBuf::from("/dev/disk/by-partlabel/osp1"),
                "root-a".to_owned() => PathBuf::from("/dev/disk/by-partlabel/osp2"),
                "root-b".to_owned() => PathBuf::from("/dev/disk/by-partlabel/osp3"),
            },
            ab_active_volume: Some(AbVolumeSelection::VolumeA),
            ..Default::default()
        };
        assert!(
            block_devices_needing_fs_creation(&ctx).unwrap().is_empty(),
            "No filesystems should be created for adopted partitions"
        );
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;

    use const_format::formatcp;

    use osutils::{
        dependencies::Dependency,
        filesystems::MountFileSystemType,
        lsblk, mount,
        testutils::repart::{self, TEST_DISK_DEVICE_PATH},
    };
    use pytest_gen::functional_test;

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
        let block_device = lsblk::get(Path::new(TEST_DISK_DEVICE_PATH)).unwrap();

        // Find the current FS on /dev/sdb
        assert_eq!(
            block_device.fstype.as_ref().unwrap(),
            "ext4",
            "Filesystem type on /dev/sdb is not ext4"
        );

        // Create /mnt/sdb if does not exist and confirm that /dev/sdb can be mounted
        Dependency::Mkdir
            .cmd()
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

        let error_string = result.as_ref().unwrap_err().root_cause().to_string();
        assert!(
            error_string.contains(&format!(
                "The file {}2 does not exist and no size was specified",
                TEST_DISK_DEVICE_PATH
            )),
            "Unexpected output: {error_string}"
        );
    }
}
