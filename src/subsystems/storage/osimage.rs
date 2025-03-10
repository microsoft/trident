use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

use const_format::formatcp;
use log::{debug, error, trace, warn};

use osutils::{df, lsblk};
use trident_api::{
    config::{FileSystemSource, FileSystemType, HostConfiguration},
    constants::{internal_params::DISABLE_FS_BLOCK_DEVICE_SIZE_CHECK, BOOT_MOUNT_POINT_PATH},
    error::{InternalError, InvalidInputError, ReportError, ServicingError, TridentError},
    primitives::bytes::ByteCount,
    status::{AbVolumeSelection, ServicingType},
};

use crate::{
    engine::boot::ESP_EXTRACTION_DIRECTORY,
    osimage::{OsImage, OsImageFileSystemType},
};

use super::EngineContext;

/// Checks if the filesystem types in the OS image and the Host Configuration match.
fn check_fs_match(a: FileSystemType, b: OsImageFileSystemType) -> bool {
    match (a, b) {
        (FileSystemType::Auto, _) => true,
        (FileSystemType::Ext4, OsImageFileSystemType::Ext4) => true,
        (FileSystemType::Vfat, OsImageFileSystemType::Vfat) => true,
        (FileSystemType::Ntfs, OsImageFileSystemType::Ntfs) => true,
        (FileSystemType::Iso9660, OsImageFileSystemType::Iso9660) => true,
        (FileSystemType::Xfs, OsImageFileSystemType::Xfs) => true,
        // Any mis-matching should be considered a failure
        (
            FileSystemType::Ext4
            | FileSystemType::Vfat
            | FileSystemType::Ntfs
            | FileSystemType::Iso9660
            | FileSystemType::Xfs,
            _,
        ) => false,
        // Host Configuration filesystem types Other, Swap, Tmpfs, and Overlay
        // do not map to any OS image filesystem types
        (FileSystemType::Other, _) => false,
        (FileSystemType::Swap, _) => false,
        (FileSystemType::Tmpfs, _) => false,
        (FileSystemType::Overlay, _) => false,
    }
}

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
    validate_filesystems(os_image, &ctx.spec)?;

    debug!("Validating uniqueness of OS image filesystem UUIDs");
    validate_filesystem_uniqueness(os_image, ctx)?;

    debug!("Validating Host Configuration root verity configuration against OS image");
    validate_root_verity_match(os_image, &ctx.spec)?;

    debug!("Validating ESP image in OS image");
    validate_esp(os_image)?;

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
fn validate_filesystems(
    os_image: &OsImage,
    host_config: &HostConfiguration,
) -> Result<(), TridentError> {
    // Populate hashmap with filesystems from OS image
    let all_os_image_filesystems = os_image
        .filesystems()
        .chain(os_image.esp_filesystem())
        .collect::<Vec<_>>();
    let os_image_filesystems_map = all_os_image_filesystems
        .iter()
        .map(|fs| (fs.mount_point.as_path(), fs.fs_type))
        .collect::<HashMap<&Path, OsImageFileSystemType>>();

    // Populate hashmap with filesystems from Host Configuration
    let hc_filesystems_map = host_config
        .storage
        .filesystems
        .iter()
        .filter(|fs| fs.source == FileSystemSource::Image)
        .map(|fs| {
            let mount_point = fs
                .mount_point
                .as_ref()
                .map(|mp| mp.path.as_path())
                .structured(InternalError::GetMountPointForOsImage)?;
            Ok((mount_point, fs.fs_type))
        })
        .collect::<Result<HashMap<_, _>, TridentError>>()?;

    // Create sets of mount points to check for missing or unused filesystems
    let os_image_filesystems_set = os_image_filesystems_map.keys().collect::<HashSet<_>>();
    let hc_filesystems_set = hc_filesystems_map.keys().collect::<HashSet<_>>();

    // Check that all filesystems in OS image are present in Host Config
    if let Some(not_found_in_hc) = os_image_filesystems_set
        .difference(&hc_filesystems_set)
        .next()
    {
        return Err(TridentError::new(
            InvalidInputError::UnusedOsImageFilesystem {
                mount_point: not_found_in_hc.display().to_string(),
                fs_type: os_image_filesystems_map[*not_found_in_hc].to_string(),
            },
        ));
    }

    // Check that all filesystems in Host Config are present in OS image
    if let Some(not_found_in_os_img) = hc_filesystems_set
        .difference(&os_image_filesystems_set)
        .next()
    {
        return Err(TridentError::new(
            InvalidInputError::MissingOsImageFilesystem {
                mount_point: not_found_in_os_img.display().to_string(),
                fs_type: hc_filesystems_map[*not_found_in_os_img].to_string(),
            },
        ));
    }

    // Check for mismatched filesystems, i.e. mount point exists in both OS image and Host
    // Configuration but filesystem type differs
    if let Some((mount_point, hc_fs_type)) =
        hc_filesystems_map.iter().find(|(mount_point, hc_fs_type)| {
            !check_fs_match(**hc_fs_type, os_image_filesystems_map[*mount_point])
        })
    {
        return Err(TridentError::new(InvalidInputError::MismatchedFsType {
            mount_point: mount_point.display().to_string(),
            hc_fs_type: hc_fs_type.to_string(),
            os_img_fs_type: os_image_filesystems_map[*mount_point].to_string(),
        }));
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

/// Validates that the OS Image and the HC match in terms of root verity configuration.
///
/// Either both must have root verity enabled or both must have it disabled.
fn validate_root_verity_match(
    os_image: &OsImage,
    host_config: &HostConfiguration,
) -> Result<(), TridentError> {
    // We validate that the OsImage has a root filesystem in previous validation steps.
    let Some(root_fs) = os_image.root_filesystem() else {
        trace!("No root filesystem found in OS image, skipping root verity validation");
        return Ok(());
    };

    let hc_verity_status = host_config.storage.has_verity_device();

    if hc_verity_status == root_fs.has_verity() {
        trace!(
            "Root verity status matches between OS image and Host Configuration: {}",
            if hc_verity_status {
                "enabled"
            } else {
                "disabled"
            }
        );
        Ok(())
    } else {
        Err(TridentError::new(InvalidInputError::RootVerityMismatch {
            hc_verity_status,
        }))
    }
}

/// Validates ESP image.
///
/// Checks that the ESP filesystem never has its verity entry populated. In addition, checks that
/// there is enough space in /tmp to perform file-based copy of ESP image, and warns the user if not
/// (this will not produce a fatal error).
fn validate_esp(os_image: &OsImage) -> Result<(), TridentError> {
    let Ok(esp_img) = os_image.esp_filesystem() else {
        trace!("Unable to access ESP filesystem.");
        return Ok(());
    };

    // Ensure there is no verity hash attached
    if esp_img.has_verity() {
        return Err(TridentError::new(InvalidInputError::UnexpectedVerityOnEsp));
    }

    // Ensure there is enough space in /tmp to perform file-based copy of ESP image
    let Ok(available_space) = df::available_space_in_fs(ESP_EXTRACTION_DIRECTORY) else {
        warn!("Failed to check if there is enough space available on '{ESP_EXTRACTION_DIRECTORY}' to copy ESP image.");
        return Ok(());
    };
    trace!("Found {available_space} bytes of available space in {ESP_EXTRACTION_DIRECTORY}.");

    let esp_img_size = esp_img.image_file.uncompressed_size;
    trace!("The uncompressed size of the ESP image is {esp_img_size} bytes.");

    if esp_img_size >= available_space {
        error!(
            "There is not enough space to copy the ESP image into {ESP_EXTRACTION_DIRECTORY}. The \
            uncompressed size of the ESP image is {}, while {ESP_EXTRACTION_DIRECTORY} has {} available.",
            ByteCount::from(esp_img_size).to_human_readable_approx(),
            ByteCount::from(available_space).to_human_readable_approx()
        );
    } else if esp_img_size >= available_space / 2 {
        warn!(
            "There may not be enough space to copy the ESP image into {ESP_EXTRACTION_DIRECTORY}. \
            The uncompressed size of the ESP image is {}, while {ESP_EXTRACTION_DIRECTORY} has {} available.",
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
                    .map_or(false, |mp| mp.path == fs.mount_point)
            })
            // We should never get here because of the checks in validate_filesystems()
            // i.e. Every mount point in the image should match to a mount point in the HC
            .structured(InternalError::GetMountPointForOsImage)?;
        let Some(device_id) = &hc_fs.device_id else {
            return Err(TridentError::new(InternalError::Internal(
                "Failed to retrieve device id for filesystem.",
            )));
        };
        let fs_size = fs.image_file.uncompressed_size;
        trace!("The size of the filesystem associated with block device '{device_id}' is {fs_size} bytes.");

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
                    device_id: device_id.to_string(),
                    min_size: fs_size,
                },
            ));
        };
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{path::PathBuf, str::FromStr};
    use url::Url;
    use uuid::Uuid;

    use osutils::{
        arch::SystemArchitecture, osrelease::OsRelease, osuuid::OsUuid,
        partition_types::DiscoverablePartitionType,
    };
    use trident_api::{
        config::{FileSystem, FileSystemSource, FileSystemType, MountPoint, Storage, VerityDevice},
        constants::{ESP_MOUNT_POINT_PATH, ROOT_MOUNT_POINT_PATH},
        error::ErrorKind,
    };

    use crate::osimage::{
        mock::{MockImage, MockOsImage, MockVerity},
        OsImage, OsImageFileSystemType,
    };

    const OSIMAGE_DUMMY_SOURCE: &str = "http://example/osimage";

    fn generate_test_host_config(
        fs: impl Iterator<Item = (&'static str, FileSystemType)>,
    ) -> HostConfiguration {
        HostConfiguration {
            storage: Storage {
                filesystems: fs
                    .map(|(path, fs_type)| FileSystem {
                        device_id: Some("dev".into()),
                        fs_type,
                        source: FileSystemSource::Image,
                        mount_point: Some(MountPoint::from_str(path).unwrap()),
                    })
                    .collect::<Vec<_>>(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn test_validate_esp_success() {
        // Generate mock OS image
        let mock_image = MockOsImage {
            source: Url::parse(OSIMAGE_DUMMY_SOURCE).unwrap(),
            os_arch: SystemArchitecture::X86,
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
        };

        // Expect validation to succeed
        validate_esp(&OsImage::mock(mock_image)).unwrap();
    }

    #[test]
    fn test_validate_esp_failure() {
        // Generate mock OS image
        let mock_image = MockOsImage {
            source: Url::parse(OSIMAGE_DUMMY_SOURCE).unwrap(),
            os_arch: SystemArchitecture::X86,
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
        };

        // Expect validation to fail
        let err = validate_esp(&OsImage::mock(mock_image)).unwrap_err();
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
            os_arch: SystemArchitecture::X86,
            os_release: OsRelease::default(),
            images: vec![MockImage {
                mount_point: PathBuf::from(BOOT_MOUNT_POINT_PATH),
                fs_type: OsImageFileSystemType::Vfat,
                fs_uuid: OsUuid::Uuid(Uuid::new_v4()),
                part_type: DiscoverablePartitionType::Xbootldr,
                verity: None,
            }],
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
    fn test_validate_root_verity_match() {
        // Generate mock OS image
        let mock_image = MockOsImage {
            source: Url::parse(OSIMAGE_DUMMY_SOURCE).unwrap(),
            os_arch: SystemArchitecture::X86,
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
        };

        // Create an OS image with root verity disabled
        let os_image_no_verity = OsImage::mock({
            let mut mock_image_clone = mock_image.clone();
            mock_image_clone.images[1].verity = None;
            mock_image_clone
        });

        // Create an OS image with root verity enabled
        let os_image_verity = OsImage::mock(mock_image);

        // HC with root verity enabled
        let host_config_verity = HostConfiguration {
            storage: Storage {
                verity: vec![VerityDevice::default()],
                ..Default::default()
            },
            ..Default::default()
        };

        // HC with root verity disabled
        let host_config_no_verity = HostConfiguration::default();

        // Test root verity:
        // OS Image: enabled
        // HC: enabled
        // Expected: OK
        validate_root_verity_match(&os_image_verity, &host_config_verity).unwrap();

        // Test root verity:
        // OS Image: disabled
        // HC: disabled
        // Expected: OK
        validate_root_verity_match(&os_image_no_verity, &host_config_no_verity).unwrap();

        // Test root verity:
        // OS Image: enabled
        // HC: disabled
        // Expected: Err
        let err = validate_root_verity_match(&os_image_verity, &host_config_no_verity).unwrap_err();
        assert_eq!(
            err.kind(),
            &ErrorKind::InvalidInput(InvalidInputError::RootVerityMismatch {
                hc_verity_status: false
            }),
            "Expected RootVerityMismatch error"
        );

        // Test root verity:
        // OS Image: disabled
        // HC: enabled
        // Expected: Err
        let err = validate_root_verity_match(&os_image_no_verity, &host_config_verity).unwrap_err();
        assert_eq!(
            err.kind(),
            &ErrorKind::InvalidInput(InvalidInputError::RootVerityMismatch {
                hc_verity_status: true
            }),
            "Expected RootVerityMismatch error"
        );
    }

    #[test]
    fn test_validate_host_config_success() {
        let mock_entries = [
            ("/image/path/A", "ext4", FileSystemType::Ext4),
            ("/image/path/B", "ext4", FileSystemType::Ext4),
        ]
        .into_iter();

        // Generate mock OS image
        let os_image = OsImage::mock(MockOsImage {
            source: Url::parse(OSIMAGE_DUMMY_SOURCE).unwrap(),
            os_arch: SystemArchitecture::X86,
            os_release: OsRelease::default(),
            images: mock_entries
                .clone()
                .map(|(path, fs_type, _)| MockImage {
                    mount_point: PathBuf::from(path),
                    fs_type: serde_json::from_str(&format!("\"{}\"", fs_type)).unwrap(),
                    fs_uuid: OsUuid::Uuid(Uuid::new_v4()),
                    part_type: DiscoverablePartitionType::LinuxGeneric,
                    verity: None,
                })
                .collect(),
        });

        // Generate HC from mock entries
        let host_config =
            generate_test_host_config(mock_entries.map(|(path, _, fs_type)| (path, fs_type)));

        // Test that validation passes
        validate_filesystems(&os_image, &host_config).unwrap();
    }

    /// This test checks the scenario where there are more filesystems listed in the OS image than
    /// there are in the Host Configuration
    #[test]
    fn test_validate_host_config_failure_unused() {
        let mock_entries_os_image = [
            ("/image/path/A", "ext4"),
            ("/image/path/B", "ext4"),
            ("/unused/image/C", "ext4"),
        ]
        .into_iter();

        // Generate mock OS image
        let os_image = OsImage::mock(MockOsImage {
            source: Url::parse(OSIMAGE_DUMMY_SOURCE).unwrap(),
            os_arch: SystemArchitecture::X86,
            os_release: OsRelease::default(),
            images: mock_entries_os_image
                .clone()
                .map(|(path, fs_type)| MockImage {
                    mount_point: PathBuf::from(path),
                    fs_type: serde_json::from_str(&format!("\"{}\"", fs_type)).unwrap(),
                    fs_uuid: OsUuid::Uuid(Uuid::new_v4()),
                    part_type: DiscoverablePartitionType::LinuxGeneric,
                    verity: None,
                })
                .collect(),
        });

        let mock_entries_hc = [
            ("/image/path/A", FileSystemType::Ext4),
            ("/image/path/B", FileSystemType::Ext4),
        ]
        .into_iter();

        // Generate Engine Context and Host Configuration
        let host_config = generate_test_host_config(mock_entries_hc);

        // Test that validation does not pass
        let validation_err = validate_filesystems(&os_image, &host_config).unwrap_err();
        assert_eq!(
            validation_err.kind(),
            &ErrorKind::InvalidInput(InvalidInputError::UnusedOsImageFilesystem {
                mount_point: "/unused/image/C".to_string(),
                fs_type: "ext4".to_string()
            }),
            "Expected UnusedOsImageFilesystem error"
        );
    }

    /// This test checks the scenario where the filesystems on the OS image do not match those in
    /// the Host Configuration
    #[test]
    fn test_validate_host_config_failure_mismatch() {
        let mock_entries_os_image =
            [("/image/path/A", "ext4"), ("/image/path/B", "ext4")].into_iter();

        // Generate mock OS image
        let os_image = OsImage::mock(MockOsImage {
            source: Url::parse(OSIMAGE_DUMMY_SOURCE).unwrap(),
            os_arch: SystemArchitecture::X86,
            os_release: OsRelease::default(),
            images: mock_entries_os_image
                .clone()
                .map(|(path, fs_type)| MockImage {
                    mount_point: PathBuf::from(path),
                    fs_type: serde_json::from_str(&format!("\"{}\"", fs_type)).unwrap(),
                    fs_uuid: OsUuid::Uuid(Uuid::new_v4()),
                    part_type: DiscoverablePartitionType::LinuxGeneric,
                    verity: None,
                })
                .collect(),
        });

        let mock_entries_hc = [
            ("/image/path/A", FileSystemType::Ext4),
            ("/image/path/B", FileSystemType::Vfat),
        ]
        .into_iter();

        // Generate Engine Context and Host Configuration
        let host_config = generate_test_host_config(mock_entries_hc);

        // Test that validation does not pass
        let validation_err = validate_filesystems(&os_image, &host_config).unwrap_err();
        assert_eq!(
            validation_err.kind(),
            &ErrorKind::InvalidInput(InvalidInputError::MismatchedFsType {
                mount_point: "/image/path/B".to_string(),
                hc_fs_type: "vfat".to_string(),
                os_img_fs_type: "ext4".to_string()
            }),
            "Expected MismatchedFsType error"
        )
    }

    /// This test checks the scenario where a filesystem on the Host Configuration is missing from
    /// the OS image
    #[test]
    fn test_validate_host_config_failure_missing() {
        let mock_entries_os_image =
            [("/image/path/A", "ext4"), ("/image/path/B", "ext4")].into_iter();

        // Generate mock OS image
        let os_image = OsImage::mock(MockOsImage {
            source: Url::parse(OSIMAGE_DUMMY_SOURCE).unwrap(),
            os_arch: SystemArchitecture::X86,
            os_release: OsRelease::default(),
            images: mock_entries_os_image
                .clone()
                .map(|(path, fs_type)| MockImage {
                    mount_point: PathBuf::from(path),
                    fs_type: serde_json::from_str(&format!("\"{}\"", fs_type)).unwrap(),
                    fs_uuid: OsUuid::Uuid(Uuid::new_v4()),
                    part_type: DiscoverablePartitionType::LinuxGeneric,
                    verity: None,
                })
                .collect(),
        });

        let mock_entries_hc = [
            ("/image/path/A", FileSystemType::Ext4),
            ("/image/path/B", FileSystemType::Ext4),
            ("/image/path/C", FileSystemType::Ext4),
        ]
        .into_iter();

        // Generate Engine Context and Host Configuration
        let host_config = generate_test_host_config(mock_entries_hc);

        // Test that validation does not pass
        let validation_err = validate_filesystems(&os_image, &host_config).unwrap_err();
        assert_eq!(
            validation_err.kind(),
            &ErrorKind::InvalidInput(InvalidInputError::MissingOsImageFilesystem {
                mount_point: "/image/path/C".to_string(),
                fs_type: "ext4".to_string()
            }),
            "Expected MissingOsImageFilesystem error"
        )
    }

    #[test]
    fn test_check_fs_match() {
        // Check success
        assert!(check_fs_match(
            FileSystemType::Ext4,
            OsImageFileSystemType::Ext4
        ));
        assert!(check_fs_match(
            FileSystemType::Vfat,
            OsImageFileSystemType::Vfat
        ));
        assert!(check_fs_match(
            FileSystemType::Ntfs,
            OsImageFileSystemType::Ntfs
        ));
        assert!(check_fs_match(
            FileSystemType::Iso9660,
            OsImageFileSystemType::Iso9660
        ));
        assert!(check_fs_match(
            FileSystemType::Xfs,
            OsImageFileSystemType::Xfs
        ));
        assert!(check_fs_match(
            FileSystemType::Auto,
            OsImageFileSystemType::Msdos
        ));

        // Check failure
        assert!(!check_fs_match(
            FileSystemType::Other,
            OsImageFileSystemType::Vfat
        ));
        assert!(!check_fs_match(
            FileSystemType::Swap,
            OsImageFileSystemType::Ntfs
        ));
        assert!(!check_fs_match(
            FileSystemType::Tmpfs,
            OsImageFileSystemType::Msdos
        ));
        assert!(!check_fs_match(
            FileSystemType::Overlay,
            OsImageFileSystemType::Ext2
        ));
    }
}
