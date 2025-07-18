use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use const_format::formatcp;
use log::{debug, error, trace, warn};

use osutils::lsblk;
use trident_api::{
    config::FileSystemSource,
    constants::{
        internal_params::{ALLOW_UNUSED_FILESYSTEMS_IN_COSI, DISABLE_FS_BLOCK_DEVICE_SIZE_CHECK},
        BOOT_MOUNT_POINT_PATH,
    },
    error::{
        InternalError, InvalidInputError, ReportError, ServicingError, TridentError,
        TridentResultExt,
    },
    primitives::bytes::ByteCount,
    status::{AbVolumeSelection, ServicingType},
    storage_graph::graph::StorageGraph,
};

use crate::{
    engine::boot::ESP_EXTRACTION_DIRECTORY,
    osimage::{OsImage, OsImageFileSystemType},
};

use super::EngineContext;

/// Validates that the Host Configuration aligns with the OS image metadata.
///
/// Checks that:
/// - There must be an equal number of filesystems in the OS image and Host Configuration
/// - Filesystems in the OS image must match on mount points with filesystems in the Host
///   Configuration
/// - The OS image and the Host Configuration match in terms of root verity configuration
/// - ESP image never has verity enabled
/// - There is enough space to copy over the ESP image into /tmp
pub fn validate_host_config(ctx: &EngineContext) -> Result<(), TridentError> {
    let Some(os_image) = &ctx.image else {
        return Ok(());
    };

    debug!("Validating Host Configuration filesystems against OS image");
    validate_filesystems(os_image, ctx)?;

    debug!("Validating uniqueness of OS image filesystem UUIDs");
    validate_filesystem_uniqueness(os_image, ctx)?;

    debug!("Validating Host Configuration verity configuration against OS image");
    validate_verity_match(os_image, &ctx.storage_graph)?;

    debug!("Validating ESP image in OS image");
    validate_esp(os_image, ctx)?;

    debug!("Validating filesystem mounted at or containing /boot");
    ensure_boot_is_ext4(os_image)?;

    if !ctx
        .spec
        .internal_params
        .get_flag(DISABLE_FS_BLOCK_DEVICE_SIZE_CHECK)
    {
        debug!("Validating filesystems in OS image fit in their corresponding block devices");
        validate_filesystem_blkdev_fit(os_image, ctx)?;
    };

    Ok(())
}

/// Validates that all OS Image filesystems are used, and that all applicable Host Configuration
/// filesystems can be satisfied by the OS image.
fn validate_filesystems(os_image: &OsImage, ctx: &EngineContext) -> Result<(), TridentError> {
    // Populate hashmap with filesystems from OS image
    let all_os_image_filesystems = os_image
        .filesystems()
        .chain(os_image.esp_filesystem())
        .collect::<Vec<_>>();
    let os_image_filesystems_set = all_os_image_filesystems
        .iter()
        .map(|fs| fs.mount_point.as_path())
        .collect::<HashSet<&Path>>();

    // Populate hashmap with ALL filesystems from Host Configuration
    let all_hc_filesystems_map = ctx
        .spec
        .storage
        .filesystems
        .iter()
        .filter(|fs| fs.source == FileSystemSource::Image)
        .map(|fs| {
            let mount_point = fs
                .mount_point
                .as_ref()
                .map(|mp| mp.path.as_path())
                .structured(InternalError::GetMountPointForFilesystemFromImage(
                    fs.description(),
                ))?;
            let device_id = fs.device_id.as_ref().structured(
                InternalError::GetDeviceIdForFilesystemFromImage(fs.description()),
            )?;
            Ok((mount_point, (fs, device_id)))
        })
        .collect::<Result<HashMap<_, _>, TridentError>>()?;

    // Create sets of mount points to check for missing or unused filesystems
    // let os_image_filesystems_set = os_image_filesystems_map.keys().collect::<HashSet<_>>();
    let all_hc_filesystems_set = all_hc_filesystems_map
        .keys()
        .copied()
        .collect::<HashSet<_>>();

    // Check that all filesystems in OS image are present in Host Config.
    // Because we are comparing against _all_ filesystems sourced from an image,
    // we should rarely hit this, if ever. It most likely means that the user
    // missed a filesystems in the HC.
    for not_found_in_hc in os_image_filesystems_set.difference(&all_hc_filesystems_set) {
        if ctx
            .spec
            .internal_params
            .get_flag(ALLOW_UNUSED_FILESYSTEMS_IN_COSI)
        {
            warn!(
                "Filesystem '{}' in OS image is not used in Host Configuration, but \
                '{ALLOW_UNUSED_FILESYSTEMS_IN_COSI}' is set, continuing.",
                not_found_in_hc.display()
            );
            continue;
        }

        return Err(TridentError::new(
            InvalidInputError::UnusedOsImageFilesystem {
                mount_point: not_found_in_hc.display().to_string(),
            },
        ));
    }

    // Get a list of all filesystems we ACTUALLY require for this specific servicing operation.
    let required_hc_filesystems_map = all_hc_filesystems_map
        .into_iter()
        .filter(|(_, (fs, device_id))| {
            // We ALWAYS require ESP to be present in the OS image.
            if fs.is_esp() {
                return true;
            }

            // If we are doing an A/B update, discard all filesystems that are NOT on AB-capable devices.
            if ctx.servicing_type == ServicingType::AbUpdate
                && !ctx
                    .storage_graph
                    .has_ab_capabilities(device_id)
                    .unwrap_or(false)
            {
                return false;
            }

            true
        })
        .collect::<HashMap<_, _>>();

    let required_hc_filesystems_set = required_hc_filesystems_map
        .keys()
        .copied()
        .collect::<HashSet<_>>();

    // Check that all required filesystems are present in OS image
    if let Some(not_found_in_os_img) = required_hc_filesystems_set
        .difference(&os_image_filesystems_set)
        .next()
    {
        return Err(TridentError::new(
            InvalidInputError::MissingOsImageFilesystem {
                mount_point: not_found_in_os_img.display().to_string(),
            },
        ));
    }

    Ok(())
}

/// Validates that all filesystems within an OS image have unique FS UUIDs. Additionally, validates
/// that A/B volume pairs have distinct FS UUIDs.
fn validate_filesystem_uniqueness(
    os_image: &OsImage,
    ctx: &EngineContext,
) -> Result<(), TridentError> {
    // Check that there are no shared filesystem UUIDs in current set of images
    // Note: purposefully do not check ESP filesystem here
    let mut current_fs_uuids = HashSet::new();
    for fs in os_image.filesystems() {
        if !current_fs_uuids.insert(fs.fs_uuid.clone()) {
            return Err(TridentError::new(InvalidInputError::DuplicateFsUuid {
                uuid: fs.fs_uuid.to_string(),
            }));
        }
    }

    // For A/B Update, check that no A/B volumes share filesystem UUIDs
    if ctx.servicing_type == ServicingType::AbUpdate {
        if let Some(ab) = &ctx.spec.storage.ab_update {
            for pair in ab.volume_pairs.iter() {
                if let Some(mp_info) = ctx.spec.storage.device_id_to_mount_point_info(&pair.id) {
                    if let Some(os_image_fs) = os_image
                        .filesystems()
                        .find(|f| f.mount_point == *mp_info.mount_point.path)
                    {
                        let inactive_volume_fs_uuid = os_image_fs.fs_uuid;

                        // Get the filesystem UUID for the currently active volume
                        let block_device_path =
                            if Some(AbVolumeSelection::VolumeA) == ctx.ab_active_volume {
                                ctx.get_block_device_path(&pair.volume_a_id).structured(
                                    ServicingError::GetBlockDevicePath {
                                        device_id: pair.id.to_string(),
                                    },
                                )?
                            } else {
                                ctx.get_block_device_path(&pair.volume_b_id).structured(
                                    ServicingError::GetBlockDevicePath {
                                        device_id: pair.id.to_string(),
                                    },
                                )?
                            };

                        let output = lsblk::get(block_device_path).structured(
                            ServicingError::GetDeviceInformation {
                                device_id: pair.volume_a_id.clone(),
                            },
                        )?;

                        if let Some(active_volume_fs_uuid) = output.fsuuid {
                            trace!("Checking A/B volume pair '{}'. Found active volume filesystem UUID '{active_volume_fs_uuid}'\
                                    and inactive volume filesystem UUID '{inactive_volume_fs_uuid}'", pair.id);
                            if active_volume_fs_uuid == inactive_volume_fs_uuid {
                                return Err(TridentError::new(
                                    InvalidInputError::DuplicateFsUuidAbUpdate {
                                        pair_id: pair.id.to_string(),
                                        uuid: inactive_volume_fs_uuid.to_string(),
                                    },
                                ));
                            }
                        } else {
                            warn!("Could not find filesystem UUID for active volume of A/B volume pair '{}'", pair.id);
                        }
                    }
                };
            }
        }
    }
    Ok(())
}

/// Validates that the OS Image and the HC match in terms of verity configuration.
///
/// The set of verity filesystems provided by the image must exactly match the set of verity
/// filesystems provided by the HC. This function will return an error if the two sets do not match.
fn validate_verity_match(
    os_image: &OsImage,
    storage_graph: &StorageGraph,
) -> Result<(), TridentError> {
    // Get a list of all the mount points with verity in the image. We can
    // collect into a hash set because the mount points should already be
    // unique.
    let img_verity_mount_points = os_image
        .filesystems()
        .filter(|fs| fs.has_verity())
        .map(|fs| fs.mount_point)
        .collect::<HashSet<_>>();

    // Now we get the same for the HC.
    let hc_verity_mount_points = storage_graph
        .filesystems_on_verity()
        .filter_map(|fs| fs.mount_point.as_ref().map(|mp| mp.path.clone()))
        .collect::<HashSet<_>>();

    // Now we can compare the two sets.
    if img_verity_mount_points != hc_verity_mount_points {
        return Err(TridentError::new(InvalidInputError::VerityMismatch {
            hc_verity_fs: hc_verity_mount_points
                .into_iter()
                .map(|mp| mp.to_string_lossy().to_string())
                .collect(),
            img_verity_fs: img_verity_mount_points
                .into_iter()
                .map(|mp| mp.to_string_lossy().to_string())
                .collect(),
        }));
    }

    Ok(())
}

/// Validates ESP image.
///
/// Checks that the ESP filesystem never has its verity entry populated. In addition, checks that
/// there is enough space in /tmp to perform file-based copy of ESP image, and warns the user if not
/// (this will not produce a fatal error).
fn validate_esp(os_image: &OsImage, ctx: &EngineContext) -> Result<(), TridentError> {
    let Ok(esp_img) = os_image.esp_filesystem() else {
        trace!("Unable to access ESP filesystem.");
        return Ok(());
    };

    // Ensure there is no verity hash attached
    if esp_img.has_verity() {
        return Err(TridentError::new(InvalidInputError::UnexpectedVerityOnEsp));
    }

    let Some(available_space) = ctx.filesystem_block_device_size(ESP_EXTRACTION_DIRECTORY) else {
        warn!("Failed to check if there is enough space available on '{ESP_EXTRACTION_DIRECTORY}' to copy ESP image.");
        return Ok(());
    };

    trace!("Found {available_space} bytes of available space in {ESP_EXTRACTION_DIRECTORY}.");

    let esp_img_size = esp_img.image_file.uncompressed_size;
    trace!("The uncompressed size of the ESP image is {esp_img_size} bytes.");

    if esp_img_size >= available_space {
        error!(
            "There is not enough space to copy the ESP image into '{ESP_EXTRACTION_DIRECTORY}'. The \
            uncompressed size of the ESP image is {}, while '{ESP_EXTRACTION_DIRECTORY}' has {} available.",
            ByteCount::from(esp_img_size).to_human_readable_approx(),
            ByteCount::from(available_space).to_human_readable_approx()
        );
    } else if esp_img_size >= available_space / 2 {
        warn!(
            "There may not be enough space to copy the ESP image into '{ESP_EXTRACTION_DIRECTORY}'. \
            The uncompressed size of the ESP image is {}, while '{ESP_EXTRACTION_DIRECTORY}' has {} available.",
            ByteCount::from(esp_img_size).to_human_readable_approx(),
            ByteCount::from(available_space).to_human_readable_approx()
        );
    }

    Ok(())
}

/// Ensures that /boot is on a filesystem of type Ext4.
fn ensure_boot_is_ext4(os_image: &OsImage) -> Result<(), TridentError> {
    // Find the filesystem containing /boot, if one exists
    let boot_fs = os_image
        .path_to_filesystem(Path::new(BOOT_MOUNT_POINT_PATH))
        .structured(InternalError::Internal(formatcp!(
            "Could not find filesystem containing {BOOT_MOUNT_POINT_PATH}",
        )))?;

    // Ensure that the filesystem containing /boot is of type Ext4. While it is
    // expected that all filesystems on an OS image should have a filesystem
    // UUID, at this time we can ensure that Trident is able to find and
    // handle only Ext4 filesystem UUIDs.
    if boot_fs.fs_type != OsImageFileSystemType::Ext4 {
        return Err(TridentError::new(
            InvalidInputError::UnsupportedBootFileSystemType {
                mount_point: boot_fs.mount_point.display().to_string(),
                fs_type: boot_fs.fs_type.to_string(),
            },
        ));
    }

    Ok(())
}

/// Validates the sizes of filesystems.
///
/// This function checks that each filesystem in the OS image fits in its corresponding block device.
fn validate_filesystem_blkdev_fit(
    os_image: &OsImage,
    ctx: &EngineContext,
) -> Result<(), TridentError> {
    let graph = &ctx.storage_graph;
    // Iterate through each filesystem in the image
    for fs in os_image.filesystems() {
        // Find corresponding filesystem in Host Configuration by matching on mount point
        let hc_fs = ctx
            .spec
            .storage
            .filesystems
            .iter()
            .find(|hc_fs| {
                hc_fs
                    .mount_point
                    .as_ref()
                    .is_some_and(|mp| mp.path == fs.mount_point)
            })
            // We should never get here because of the checks in validate_filesystems()
            // i.e. Every mount point in the image should match to a mount point in the HC
            .structured(InternalError::Internal(
                "Failed to find previously seen filesystem in the Host Configuration.",
            ))
            .message(format!(
                "Missing filesystem mounted at '{}'",
                fs.mount_point.display()
            ))?;

        let device_id = &hc_fs
            .device_id
            .as_ref()
            .structured(InternalError::Internal(
                "Failed to retrieve deviceId for filesystem sourced from image.",
            ))
            .message(format!(
                "Filesystem [{}] is missing a deviceId.",
                hc_fs.description(),
            ))?;

        let fs_size = fs.image_file.uncompressed_size;
        trace!("The size of the filesystem associated with block device '{device_id}' is {fs_size} bytes.");

        if let Some(fs_verity) = fs.verity.as_ref() {
            // If the filesystem has a verity hash, we need to check the size of the
            // block device that will contain the verity hash.
            validate_hash_filesystem_blkdev_fit(
                fs_verity.hash_image_file.uncompressed_size,
                fs.mount_point.clone(),
                graph,
            )?;
        }

        let Some(blkdev_size) = graph.block_device_size(device_id) else {
            debug!("Could not find the size of the block device with id '{device_id}'. Block device may not have a fixed size.");
            continue;
        };

        trace!("The size of the block device with id '{device_id}' is {blkdev_size} bytes.");

        debug!(
            "Found filesystem with size {} and block device with size {} for device '{device_id}'",
            ByteCount::from(fs_size).to_human_readable_approx(),
            ByteCount::from(blkdev_size).to_human_readable_approx()
        );

        if fs_size > blkdev_size {
            return Err(TridentError::new(
                InvalidInputError::FilesystemSizeExceedsBlockDevice {
                    mount_point: fs.mount_point.display().to_string(),
                    device_id: device_id.to_string(),
                    fs_size: ByteCount::from(fs_size),
                    device_size: ByteCount::from(blkdev_size),
                },
            ));
        };
    }
    Ok(())
}

/// Validate that the size of a verity filesystem hash fits in the configured block device.
fn validate_hash_filesystem_blkdev_fit(
    fs_verity_hash_file_size: u64,
    fs_mount_point: PathBuf,
    graph: &StorageGraph,
) -> Result<(), TridentError> {
    // Get the verity block device corresponding to the filesystem
    let verity_device = graph
        .verity_device_for_filesystem(&fs_mount_point)
        .structured(InternalError::Internal(
            "No verity device found for mount point",
        ))
        .message(format!(
            "Failed to find verity device for filesystem mounted at '{}'",
            fs_mount_point.display()
        ))?;

    // Get the size of the block device configured for the verity hash
    let Some(blkdev_hash_size) = graph.block_device_size(&verity_device.hash_device_id) else {
        debug!(
            "Could not find the size of the block device with id '{}'. Block device may not have a fixed size.",
            verity_device.hash_device_id
        );
        return Ok(());
    };

    debug!(
        "Found filesystem with verity hash of size {} and block device with size {} for device '{}'",
        ByteCount::from(fs_verity_hash_file_size).to_human_readable_approx(),
        ByteCount::from(blkdev_hash_size).to_human_readable_approx(),
        &verity_device.hash_device_id,
    );

    // Ensure that the filesystem hash will fit in the block device
    if fs_verity_hash_file_size > blkdev_hash_size {
        return Err(TridentError::new(
            InvalidInputError::FilesystemSizeExceedsBlockDevice {
                mount_point: fs_mount_point.to_string_lossy().into(),
                device_id: verity_device.hash_device_id.to_string(),
                fs_size: ByteCount::from(fs_verity_hash_file_size),
                device_size: ByteCount::from(blkdev_hash_size),
            },
        ));
    };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{collections::BTreeSet, path::PathBuf, str::FromStr};

    use url::Url;
    use uuid::Uuid;

    use osutils::osrelease::OsRelease;
    use sysdefs::{
        arch::SystemArchitecture, osuuid::OsUuid, partition_types::DiscoverablePartitionType,
    };
    use trident_api::{
        config::{
            AbUpdate, AbVolumePair, Disk, FileSystem, FileSystemSource, HostConfiguration,
            MountOptions, MountPoint, Partition, PartitionSize, Storage, VerityDevice,
        },
        constants::{ESP_MOUNT_POINT_PATH, ROOT_MOUNT_POINT_PATH},
        error::ErrorKind,
        misc::IdGenerator,
    };

    use crate::osimage::{
        mock::{MockImage, MockOsImage, MockVerity},
        OsImage, OsImageFileSystemType,
    };

    const OSIMAGE_DUMMY_SOURCE: &str = "http://example/osimage";

    /// Generates a simple test context for clean install servicing type.
    ///
    /// The context includes:
    /// - A mock OS image.
    /// - A mock Host Configuration.
    /// - A built storage graph.
    fn generate_test_context<'a, 'b>(
        hc_data: impl Iterator<Item = &'a (impl AsRef<Path> + 'a)>,
        img_data: impl Iterator<Item = &'b (impl AsRef<Path> + 'b, OsImageFileSystemType)>,
    ) -> EngineContext {
        let hc_data = hc_data.map(|a| (a, false)).collect::<Vec<_>>();
        let mut ctx = generate_test_context_ab_update(hc_data.iter(), img_data);
        ctx.servicing_type = ServicingType::CleanInstall;
        ctx.ab_active_volume = None;
        ctx
    }

    /// Generates a test context for A/B update servicing type.
    ///
    /// Same as `generate_test_context`, but with the addition of a new bool parameter
    /// `on_ab` on the HC data iterator. This parameter indicates whether the filesystem
    /// should be on an A/B volume pair or not.
    fn generate_test_context_ab_update<'a, 'b>(
        hc_data: impl Iterator<Item = &'a (impl AsRef<Path> + 'a, bool)>,
        img_data: impl Iterator<Item = &'b (impl AsRef<Path> + 'b, OsImageFileSystemType)>,
    ) -> EngineContext {
        let mut partitions = Vec::new();
        let mut filesystems = Vec::new();
        let mut volume_pairs = Vec::new();

        let mut part_id_gen = IdGenerator::new("part");
        let mut abvol_id_gen = IdGenerator::new("abvol");

        let mut pushpart = || {
            let part_id = part_id_gen.next_id();
            partitions.push(Partition {
                id: part_id.clone(),
                size: PartitionSize::from_str("1G").unwrap(),
                partition_type: Default::default(),
            });
            part_id
        };

        for (mnt, on_ab) in hc_data {
            let device_id = if *on_ab {
                let part_a = pushpart();
                let part_b = pushpart();
                let abvol_id = abvol_id_gen.next_id();
                volume_pairs.push(AbVolumePair {
                    id: abvol_id.clone(),
                    volume_a_id: part_a,
                    volume_b_id: part_b,
                });
                abvol_id
            } else {
                pushpart()
            };

            filesystems.push(FileSystem {
                device_id: Some(device_id),
                source: FileSystemSource::Image,
                mount_point: Some(MountPoint {
                    path: mnt.as_ref().to_path_buf(),
                    options: MountOptions::defaults(),
                }),
            });
        }

        let hc = HostConfiguration {
            storage: Storage {
                disks: vec![Disk {
                    id: "disk0".into(),
                    device: "/dev/sda".into(),
                    partitions,
                    ..Default::default()
                }],
                filesystems,
                ab_update: Some(AbUpdate { volume_pairs }),
                ..Default::default()
            },
            ..Default::default()
        };

        EngineContext {
            storage_graph: hc.storage.build_graph().unwrap(),
            servicing_type: ServicingType::AbUpdate,
            ab_active_volume: Some(AbVolumeSelection::VolumeA),
            spec: hc,
            image: Some(OsImage::mock(MockOsImage::new().with_images(img_data.map(
                |(path, fs_type)| {
                    MockImage::new(
                        path,
                        *fs_type,
                        DiscoverablePartitionType::LinuxGeneric,
                        None::<&str>,
                    )
                },
            )))),
            ..Default::default()
        }
    }

    #[test]
    fn test_validate_filesystem_uniqueness_clean_install() {
        // Scenario 1 (failure): Duplicate FSUUIDs found in same image
        let mut mock_image = MockOsImage {
            source: Url::parse(OSIMAGE_DUMMY_SOURCE).unwrap(),
            os_arch: SystemArchitecture::Amd64,
            os_release: OsRelease::default(),
            images: vec![
                MockImage {
                    mount_point: PathBuf::from("/trident"),
                    fs_type: OsImageFileSystemType::Ext4,
                    fs_uuid: OsUuid::Uuid(
                        Uuid::parse_str("a1a2a3a4b1b2c1c2d1d2d3d4d5d6d7d8").unwrap(),
                    ),
                    part_type: DiscoverablePartitionType::LinuxGeneric,
                    verity: None,
                },
                MockImage {
                    mount_point: PathBuf::from(ROOT_MOUNT_POINT_PATH),
                    fs_type: OsImageFileSystemType::Ext4,
                    fs_uuid: OsUuid::Uuid(
                        Uuid::parse_str("a1a2a3a4b1b2c1c2d1d2d3d4d5d6d7d8").unwrap(),
                    ),
                    part_type: DiscoverablePartitionType::Root,
                    verity: None,
                },
            ],
            is_uki: false,
        };
        let os_image = OsImage::mock(mock_image.clone());
        let mut ctx = EngineContext {
            servicing_type: ServicingType::CleanInstall,
            image: Some(os_image.clone()),
            ..Default::default()
        };

        let err = validate_filesystem_uniqueness(&os_image, &ctx).unwrap_err();
        assert_eq!(
            err.kind(),
            &ErrorKind::InvalidInput(InvalidInputError::DuplicateFsUuid {
                uuid: "a1a2a3a4-b1b2-c1c2-d1d2-d3d4d5d6d7d8".to_string(),
            })
        );

        // Scenario 2 (success): Unique FSUUIDs
        mock_image.images[0].fs_uuid =
            OsUuid::Uuid(Uuid::parse_str("a1a1a1a1b1b1c1c1d1d1d1d1d1d1d1d1").unwrap());
        let os_image = OsImage::mock(mock_image.clone());
        ctx.image = Some(os_image.clone());
        validate_filesystem_uniqueness(&os_image, &ctx).unwrap();
    }

    #[test]
    fn test_validate_esp_success() {
        let ctx = EngineContext::default()
            .with_image(MockOsImage {
                source: Url::parse(OSIMAGE_DUMMY_SOURCE).unwrap(),
                os_arch: SystemArchitecture::Amd64,
                os_release: OsRelease::default(),
                images: vec![
                    MockImage {
                        mount_point: PathBuf::from(ESP_MOUNT_POINT_PATH),
                        fs_type: OsImageFileSystemType::Vfat,
                        fs_uuid: OsUuid::Uuid(Uuid::new_v4()),
                        part_type: DiscoverablePartitionType::Esp,
                        verity: None,
                    },
                    MockImage {
                        mount_point: PathBuf::from(ROOT_MOUNT_POINT_PATH),
                        fs_type: OsImageFileSystemType::Ext4,
                        fs_uuid: OsUuid::Uuid(Uuid::new_v4()),
                        part_type: DiscoverablePartitionType::Root,
                        verity: Some(MockVerity {
                            roothash: "mock-roothash".to_string(),
                        }),
                    },
                ],
                is_uki: false,
            })
            .with_spec(HostConfiguration {
                storage: Storage {
                    disks: vec![Disk {
                        id: "disk1".to_owned(),
                        device: PathBuf::from("/dev/sda"),
                        partitions: vec![Partition {
                            id: "part1".to_owned(),
                            size: 4096.into(),
                            partition_type: Default::default(),
                        }],
                        ..Default::default()
                    }],
                    filesystems: vec![FileSystem {
                        device_id: Some("part1".to_owned()),
                        mount_point: Some("/data".into()),
                        source: FileSystemSource::Image,
                    }],
                    ..Default::default()
                },
                ..Default::default()
            });

        // Expect validation to succeed
        validate_esp(ctx.image.as_ref().unwrap(), &ctx).unwrap();
    }

    #[test]
    fn test_validate_esp_failure() {
        let ctx = EngineContext::default().with_image(MockOsImage {
            source: Url::parse(OSIMAGE_DUMMY_SOURCE).unwrap(),
            os_arch: SystemArchitecture::Amd64,
            os_release: OsRelease::default(),
            images: vec![
                MockImage {
                    mount_point: PathBuf::from(ESP_MOUNT_POINT_PATH),
                    fs_type: OsImageFileSystemType::Vfat,
                    fs_uuid: OsUuid::Uuid(Uuid::new_v4()),
                    part_type: DiscoverablePartitionType::Esp,
                    verity: Some(MockVerity {
                        roothash: "mock-hash".to_string(),
                    }),
                },
                MockImage {
                    mount_point: PathBuf::from(ROOT_MOUNT_POINT_PATH),
                    fs_type: OsImageFileSystemType::Ext4,
                    fs_uuid: OsUuid::Uuid(Uuid::new_v4()),
                    part_type: DiscoverablePartitionType::Root,
                    verity: Some(MockVerity {
                        roothash: "mock-roothash".to_string(),
                    }),
                },
            ],
            is_uki: false,
        });

        // Expect validation to fail
        let err = validate_esp(ctx.image.as_ref().unwrap(), &ctx).unwrap_err();
        assert_eq!(
            err.kind(),
            &ErrorKind::InvalidInput(InvalidInputError::UnexpectedVerityOnEsp)
        );
    }

    #[test]
    fn test_ensure_boot_is_ext4() {
        // Generate mock OS image
        let mut mock_image = MockOsImage {
            source: Url::parse(OSIMAGE_DUMMY_SOURCE).unwrap(),
            os_arch: SystemArchitecture::Amd64,
            os_release: OsRelease::default(),
            images: vec![MockImage {
                mount_point: PathBuf::from(BOOT_MOUNT_POINT_PATH),
                fs_type: OsImageFileSystemType::Vfat,
                fs_uuid: OsUuid::Uuid(Uuid::new_v4()),
                part_type: DiscoverablePartitionType::Xbootldr,
                verity: None,
            }],
            is_uki: false,
        };

        // Expect validation to fail
        let err = ensure_boot_is_ext4(&OsImage::mock(mock_image.clone())).unwrap_err();
        assert_eq!(
            err.kind(),
            &ErrorKind::InvalidInput(InvalidInputError::UnsupportedBootFileSystemType {
                mount_point: "/boot".to_string(),
                fs_type: "vfat".to_string()
            })
        );

        mock_image.images[0].fs_type = OsImageFileSystemType::Ext4;

        // Expect validation to succeed
        ensure_boot_is_ext4(&OsImage::mock(mock_image)).unwrap();
    }

    #[test]
    fn test_validate_verity_match_no_verity() {
        // Generate mock OS image
        let mock_image = OsImage::mock(MockOsImage::new().with_image(MockImage::new(
            ROOT_MOUNT_POINT_PATH,
            OsImageFileSystemType::Ext4,
            DiscoverablePartitionType::LinuxGeneric,
            None::<&str>,
        )));

        let graph = Storage {
            disks: vec![Disk {
                device: "/dev/sda".into(),
                partitions: vec![
                    Partition {
                        id: "data".into(),
                        partition_type: Default::default(),
                        size: PartitionSize::from_str("1G").unwrap(),
                    },
                    Partition {
                        id: "hash".into(),
                        partition_type: Default::default(),
                        size: PartitionSize::from_str("1G").unwrap(),
                    },
                ],
                ..Default::default()
            }],
            filesystems: vec![FileSystem {
                device_id: Some("data".into()),
                source: FileSystemSource::Image,
                mount_point: Some(MountPoint::from_str(ROOT_MOUNT_POINT_PATH).unwrap()),
            }],
            ..Default::default()
        }
        .build_graph()
        .unwrap();

        validate_verity_match(&mock_image, &graph).unwrap();
    }

    #[test]
    fn test_validate_verity_match_with_verity() {
        // Generate mock OS image
        let mock_image = OsImage::mock(MockOsImage::new().with_image(MockImage::new(
            ROOT_MOUNT_POINT_PATH,
            OsImageFileSystemType::Ext4,
            DiscoverablePartitionType::LinuxGeneric,
            Some("roothash"),
        )));

        let graph = Storage {
            disks: vec![Disk {
                device: "/dev/sda".into(),
                partitions: vec![
                    Partition {
                        id: "data".into(),
                        partition_type: Default::default(),
                        size: PartitionSize::from_str("1G").unwrap(),
                    },
                    Partition {
                        id: "hash".into(),
                        partition_type: Default::default(),
                        size: PartitionSize::from_str("1G").unwrap(),
                    },
                ],
                ..Default::default()
            }],
            verity: vec![VerityDevice {
                id: "verity".into(),
                name: "verity".into(),
                data_device_id: "data".into(),
                hash_device_id: "hash".into(),
                ..Default::default()
            }],
            filesystems: vec![FileSystem {
                device_id: Some("verity".into()),
                source: FileSystemSource::Image,
                mount_point: Some(MountPoint::from_str(ROOT_MOUNT_POINT_PATH).unwrap()),
            }],
            ..Default::default()
        }
        .build_graph()
        .unwrap();

        validate_verity_match(&mock_image, &graph).unwrap();
    }

    #[test]
    fn test_validate_verity_match_with_mismatch_img() {
        // Generate mock OS image
        let mock_image = OsImage::mock(MockOsImage::new().with_image(MockImage::new(
            ROOT_MOUNT_POINT_PATH,
            OsImageFileSystemType::Ext4,
            DiscoverablePartitionType::LinuxGeneric,
            None::<&str>,
        )));

        let graph = Storage {
            disks: vec![Disk {
                device: "/dev/sda".into(),
                partitions: vec![
                    Partition {
                        id: "data".into(),
                        partition_type: Default::default(),
                        size: PartitionSize::from_str("1G").unwrap(),
                    },
                    Partition {
                        id: "hash".into(),
                        partition_type: Default::default(),
                        size: PartitionSize::from_str("1G").unwrap(),
                    },
                ],
                ..Default::default()
            }],
            verity: vec![VerityDevice {
                id: "verity".into(),
                name: "verity".into(),
                data_device_id: "data".into(),
                hash_device_id: "hash".into(),
                ..Default::default()
            }],
            filesystems: vec![FileSystem {
                device_id: Some("verity".into()),
                source: FileSystemSource::Image,
                mount_point: Some(MountPoint::from_str(ROOT_MOUNT_POINT_PATH).unwrap()),
            }],
            ..Default::default()
        }
        .build_graph()
        .unwrap();

        assert_eq!(
            validate_verity_match(&mock_image, &graph)
                .unwrap_err()
                .kind(),
            &ErrorKind::InvalidInput(InvalidInputError::VerityMismatch {
                hc_verity_fs: ["/".to_string()].into_iter().collect(),
                img_verity_fs: BTreeSet::new(),
            })
        );
    }

    #[test]
    fn test_validate_verity_match_with_mismatch_hc() {
        // Generate mock OS image
        let mock_image = OsImage::mock(MockOsImage::new().with_image(MockImage::new(
            ROOT_MOUNT_POINT_PATH,
            OsImageFileSystemType::Ext4,
            DiscoverablePartitionType::LinuxGeneric,
            Some("verity-hash"),
        )));

        let graph = Storage {
            disks: vec![Disk {
                device: "/dev/sda".into(),
                partitions: vec![
                    Partition {
                        id: "data".into(),
                        partition_type: Default::default(),
                        size: PartitionSize::from_str("1G").unwrap(),
                    },
                    Partition {
                        id: "hash".into(),
                        partition_type: Default::default(),
                        size: PartitionSize::from_str("1G").unwrap(),
                    },
                ],
                ..Default::default()
            }],
            filesystems: vec![FileSystem {
                device_id: Some("data".into()),
                source: FileSystemSource::Image,
                mount_point: Some(MountPoint::from_str(ROOT_MOUNT_POINT_PATH).unwrap()),
            }],
            ..Default::default()
        }
        .build_graph()
        .unwrap();

        assert_eq!(
            validate_verity_match(&mock_image, &graph)
                .unwrap_err()
                .kind(),
            &ErrorKind::InvalidInput(InvalidInputError::VerityMismatch {
                hc_verity_fs: BTreeSet::new(),
                img_verity_fs: ["/".to_string()].into_iter().collect(),
            })
        );
    }

    #[test]
    fn test_validate_host_config_clean_install() {
        let hc_entries = ["/mnt/path/A", "/mnt/path/B"];

        let img_entries = [
            ("/mnt/path/A", OsImageFileSystemType::Ext4),
            ("/mnt/path/B", OsImageFileSystemType::Ext4),
        ];

        // Generate HC from mock entries
        let ctx = generate_test_context(hc_entries.iter(), img_entries.iter());
        let img = ctx.image.as_ref().unwrap();

        // Test that validation passes
        validate_filesystems(img, &ctx).unwrap();
    }

    /// This test checks the scenario where there are more filesystems listed in the OS image than
    /// there are in the Host Configuration
    #[test]
    fn test_validate_host_config_failure_unused() {
        let hc_entries = ["/mnt/path/A", "/mnt/path/B"];

        let img_entries = [
            ("/mnt/path/A", OsImageFileSystemType::Ext4),
            ("/mnt/path/B", OsImageFileSystemType::Ext4),
            ("/mnt/unused/C", OsImageFileSystemType::Ext4),
        ];

        // Generate HC from mock entries
        let ctx = generate_test_context(hc_entries.iter(), img_entries.iter());
        let img = ctx.image.as_ref().unwrap();

        // Test that validation does not pass
        let validation_err = validate_filesystems(img, &ctx).unwrap_err();
        assert_eq!(
            validation_err.kind(),
            &ErrorKind::InvalidInput(InvalidInputError::UnusedOsImageFilesystem {
                mount_point: "/mnt/unused/C".to_string(),
            }),
            "Expected UnusedOsImageFilesystem error"
        );
    }

    /// This test checks the scenario where a filesystem on the Host Configuration is missing from
    /// the OS image
    #[test]
    fn test_validate_host_config_failure_missing() {
        let hc_entries = ["/mnt/path/A", "/mnt/path/B", "/mnt/path/C"];

        let img_entries = [
            ("/mnt/path/A", OsImageFileSystemType::Ext4),
            ("/mnt/path/B", OsImageFileSystemType::Ext4),
        ];

        // Generate HC from mock entries
        let ctx = generate_test_context(hc_entries.iter(), img_entries.iter());
        let img = ctx.image.as_ref().unwrap();

        // Test that validation does not pass
        let validation_err = validate_filesystems(img, &ctx).unwrap_err();
        assert_eq!(
            validation_err.kind(),
            &ErrorKind::InvalidInput(InvalidInputError::MissingOsImageFilesystem {
                mount_point: "/mnt/path/C".to_string(),
            }),
            "Expected MissingOsImageFilesystem error"
        )
    }

    #[test]
    fn test_validate_host_config_abupdate() {
        let hc_entries = [
            // These are on ab volumes, therefore expected to exist in the image.
            ("/mnt/path/A", true),
            ("/mnt/path/B", true),
            // These are NOT on ab volumes, therefore not expected to exist in the image.
            ("/mnt/path/C", false),
            ("/mnt/path/D", false),
            ("/mnt/path/E", false),
        ];

        let img_entries = [
            ("/mnt/path/A", OsImageFileSystemType::Ext4),
            ("/mnt/path/B", OsImageFileSystemType::Ext4),
            // This one is not required, but its presence should be ok.
            ("/mnt/path/C", OsImageFileSystemType::Ext4),
        ];

        // Generate HC from mock entries
        let ctx = generate_test_context_ab_update(hc_entries.iter(), img_entries.iter());
        let img = ctx.image.as_ref().unwrap();

        // Test that validation passes
        validate_filesystems(img, &ctx).unwrap();
    }

    #[test]
    fn test_validate_host_config_abupdate_failure_unused() {
        let hc_entries = [
            // These are on ab volumes, therefore expected to exist in the image.
            ("/mnt/path/A", true),
            ("/mnt/path/B", true),
            // These are NOT on ab volumes, therefore not expected to exist in the image.
            ("/mnt/path/C", false),
            ("/mnt/path/D", false),
            ("/mnt/path/E", false),
        ];

        let img_entries = [
            ("/mnt/path/A", OsImageFileSystemType::Ext4),
            ("/mnt/path/B", OsImageFileSystemType::Ext4),
            // This one should still cause a failure!
            ("/mnt/unused/C", OsImageFileSystemType::Ext4),
        ];

        // Generate HC from mock entries
        let ctx = generate_test_context_ab_update(hc_entries.iter(), img_entries.iter());
        let img = ctx.image.as_ref().unwrap();

        // Test that validation does not pass
        let validation_err = validate_filesystems(img, &ctx).unwrap_err();
        assert_eq!(
            validation_err.kind(),
            &ErrorKind::InvalidInput(InvalidInputError::UnusedOsImageFilesystem {
                mount_point: "/mnt/unused/C".to_string(),
            }),
            "Expected UnusedOsImageFilesystem error"
        );
    }

    #[test]
    fn test_validate_hash_filesystem_blkdev_fit() {
        let required_size_gb = 1;
        let required_partition_size =
            PartitionSize::from_str(format!("{required_size_gb}G").as_str()).unwrap();
        let too_big_size_gb = 2;
        let too_big_partition_size =
            PartitionSize::from_str(format!("{too_big_size_gb}G").as_str()).unwrap();
        let mount_point = "/mnt/path/verity";
        let fs_mount_point = PathBuf::from(mount_point);
        let graph = Storage {
            disks: vec![Disk {
                device: "/dev/sda".into(),
                partitions: vec![
                    Partition {
                        id: "data".into(),
                        partition_type: Default::default(),
                        size: required_partition_size,
                    },
                    Partition {
                        id: "hash".into(),
                        partition_type: Default::default(),
                        size: required_partition_size,
                    },
                ],
                ..Default::default()
            }],
            verity: vec![VerityDevice {
                id: "verity".into(),
                name: "verity".into(),
                data_device_id: "data".into(),
                hash_device_id: "hash".into(),
                ..Default::default()
            }],
            filesystems: vec![FileSystem {
                device_id: Some("verity".into()),
                source: FileSystemSource::Image,
                mount_point: Some(MountPoint::from_str(mount_point).unwrap()),
            }],
            ..Default::default()
        }
        .build_graph()
        .unwrap();

        // Test with exact matching block device size
        validate_hash_filesystem_blkdev_fit(
            required_partition_size.to_bytes().unwrap(),
            fs_mount_point.clone(),
            &graph,
        )
        .unwrap();

        // Test with too small block device size
        let err = validate_hash_filesystem_blkdev_fit(
            too_big_partition_size.to_bytes().unwrap(),
            fs_mount_point.clone(),
            &graph,
        )
        .unwrap_err();
        assert_eq!(
            err.kind(),
            &ErrorKind::InvalidInput(InvalidInputError::FilesystemSizeExceedsBlockDevice {
                mount_point: mount_point.to_string(),
                device_id: "hash".to_string(),
                fs_size: ByteCount::from(too_big_partition_size.to_bytes().unwrap()),
                device_size: ByteCount::from(required_partition_size.to_bytes().unwrap()),
            })
        );
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;

    use std::{path::PathBuf, str::FromStr};

    use maplit::btreemap;
    use url::Url;
    use uuid::Uuid;

    use osutils::{
        filesystems::MkfsFileSystemType,
        mkfs,
        osrelease::OsRelease,
        testutils::repart::{self, TEST_DISK_DEVICE_PATH},
    };
    use pytest_gen::functional_test;
    use sysdefs::{
        arch::SystemArchitecture, osuuid::OsUuid, partition_types::DiscoverablePartitionType,
    };
    use trident_api::{
        config::{AbUpdate, AbVolumePair, FileSystem, HostConfiguration, MountPoint, Storage},
        error::ErrorKind,
    };

    use crate::osimage::mock::{MockImage, MockOsImage};

    #[functional_test]
    fn test_validate_filesystem_uniqueness_update() {
        repart::clear_disk(Path::new(TEST_DISK_DEVICE_PATH)).unwrap();
        mkfs::run(Path::new(TEST_DISK_DEVICE_PATH), MkfsFileSystemType::Ext4).unwrap();
        let root_block_device = lsblk::get(Path::new(TEST_DISK_DEVICE_PATH)).unwrap();

        // Make sure filesystem was created
        assert_eq!(
            root_block_device.fstype.as_ref().unwrap(),
            "ext4",
            "Filesystem type on /dev/sdb is not ext4"
        );

        // Scenario 1 (failure): Same image used during A/B update (simulated by
        // same FSUUID as in current disk), resulting in duplicate FSUUIDs
        let mut mock_image = MockOsImage {
            source: Url::parse("http://example/osimage").unwrap(),
            os_arch: SystemArchitecture::Amd64,
            os_release: OsRelease::default(),
            is_uki: false,
            images: vec![MockImage {
                mount_point: PathBuf::from("/"),
                fs_type: OsImageFileSystemType::Ext4,
                fs_uuid: root_block_device.fsuuid.clone().unwrap(),
                part_type: DiscoverablePartitionType::Root,
                verity: None,
            }],
        };
        let os_image = OsImage::mock(mock_image.clone());
        let mut ctx = EngineContext {
            servicing_type: ServicingType::AbUpdate,
            spec: HostConfiguration {
                storage: Storage {
                    filesystems: vec![FileSystem {
                        device_id: Some("root".into()),
                        source: FileSystemSource::Image,
                        mount_point: Some(MountPoint::from_str("/").unwrap()),
                    }],
                    ab_update: Some(AbUpdate {
                        volume_pairs: vec![AbVolumePair {
                            id: "root".to_string(),
                            volume_a_id: "root-a".to_string(),
                            volume_b_id: "root-b".to_string(),
                        }],
                    }),
                    ..Default::default()
                },
                ..Default::default()
            },
            image: Some(os_image.clone()),
            ab_active_volume: Some(AbVolumeSelection::VolumeA),
            partition_paths: btreemap! {
                "root-a".into() => PathBuf::from(TEST_DISK_DEVICE_PATH),
            },
            ..Default::default()
        };
        let err = validate_filesystem_uniqueness(&os_image, &ctx).unwrap_err();
        assert_eq!(
            err.kind(),
            &ErrorKind::InvalidInput(InvalidInputError::DuplicateFsUuidAbUpdate {
                pair_id: "root".to_string(),
                uuid: root_block_device.fsuuid.unwrap().to_string(),
            })
        );

        // Scenario 2 (success): distinct OS image with distinct FSUUID
        mock_image.images[0].fs_uuid = OsUuid::Uuid(Uuid::new_v4());
        let os_image_new = OsImage::mock(mock_image);
        ctx.image = Some(os_image_new.clone());
        validate_filesystem_uniqueness(&os_image_new, &ctx).unwrap();

        // Clean up
        repart::clear_disk(Path::new(TEST_DISK_DEVICE_PATH)).unwrap();
    }
}
