use std::collections::HashMap;

use log::debug;

use trident_api::{
    config::{HostConfigurationDynamicValidationError, Image},
    error::{InvalidInputError, TridentError},
    status::ServicingType,
    BlockDeviceId,
};

use crate::engine::EngineContext;

/// Checks if the host needs an A/B update. First, compares the partition images in the specs. If
/// the partition images have not been updated, checks if the new Host Configuration requests an OS
/// image. If yes, update is needed, unless the old Host Configuration also requested an OS image
/// and the URLs and SHA256 checksums are the same.
///
/// TODO: Remove this logic for partition images once COSI becomes the default for GA.
///
/// TODO: Once hashes for OS images are introduced into Host Configuration, need to compare hashes
/// for OS images. Related ADO task:
/// https://dev.azure.com/mariner-org/ECF/_workitems/edit/10845.
pub(super) fn ab_update_required(ctx: &EngineContext) -> Result<bool, TridentError> {
    // First, check if the images have been updated
    let old_images = ctx
        .spec_old
        .storage
        .get_ab_volume_pair_images()
        .into_iter()
        .chain(ctx.spec_old.storage.get_esp_images())
        .collect();
    let new_images = ctx
        .spec
        .storage
        .get_ab_volume_pair_images()
        .into_iter()
        .chain(ctx.spec.storage.get_esp_images())
        .collect();
    let ab_update_needed = !get_updated_images(old_images, new_images).is_empty();

    // If the images have been updated, return immediately
    if ab_update_needed {
        debug!("Partition images have been updated: A/B update is required");
        return Ok(ab_update_needed);
    }

    debug!("Checking OS image to determine if an A/B update is required");
    // Otherwise, continue checking OS images
    match (&ctx.spec_old.image, &ctx.spec.image) {
        // If OS image is not requested in the new spec, no update is needed.
        (None, None) => Ok(false),

        (None, Some(_)) => {
            if ctx
                .spec_old
                .storage
                .filesystems
                .iter()
                .any(|fs| fs.source.is_old_api())
            {
                // Return an error if the old spec was provisioned with
                // partition images, but the new spec is an OS image.
                Err(TridentError::new(InvalidInputError::from(
                    HostConfigurationDynamicValidationError::DeployOsImageAfterPartitionImages,
                )))
            } else {
                // If the old spec is NOT using the old API, but did not get
                // an OS image, then, this is most likely an offline-init's
                // first update.
                Ok(true)
            }
        }

        // If OS image is requested in both specs, compare the URLs.
        (Some(old_os_image), Some(new_os_image)) => Ok(old_os_image.url != new_os_image.url),

        (Some(_), None) => {
            // Return an error if the old spec requests an OS image but the new spec does not.
            Err(TridentError::new(InvalidInputError::from(
                HostConfigurationDynamicValidationError::DeployPartitionImagesAfterOsImage,
            )))
        }
    }
}

/// Returns the images that need to be updated.
///
/// The images are compared between the old images and the new images. If an image is found in the
/// new images that is not in the old images, or if the image is found in both but the URL or SHA256
/// checksum has changed, the image is added to the list of images to be updated.
pub(crate) fn get_updated_images(
    old_images: Vec<(BlockDeviceId, Image)>,
    mut new_images: Vec<(BlockDeviceId, Image)>,
) -> Vec<(BlockDeviceId, Image)> {
    let old_images: HashMap<String, Image> = old_images.into_iter().collect();
    new_images.retain(|(device_id, image)| {
        if let Some(old_image) = old_images.get(device_id) {
            old_image.url != image.url || old_image.sha256 != image.sha256
        } else {
            true
        }
    });
    new_images
}

/// Validates that the new Host Configuration in `ctx.spec` requests deployment only of images that
/// can be deployed.
///
/// This function is called during A/B update to ensure that the Host Configuration does not request
/// Trident to re-deploy images on standalone volumes that are shared between A and B, such as
/// /var/lib/trident.
pub(super) fn validate_host_config(ctx: &EngineContext) -> Result<(), TridentError> {
    if ctx.servicing_type == ServicingType::AbUpdate {
        // Get lists of all old and new images.
        let old_images = ctx
            .spec_old
            .storage
            .get_images()
            .into_iter()
            .chain(ctx.spec_old.storage.get_esp_images())
            .collect();
        let new_images = ctx
            .spec
            .storage
            .get_images()
            .into_iter()
            .chain(ctx.spec.storage.get_esp_images())
            .collect();

        // Filter only those images that have been updated.
        let updated_images = get_updated_images(old_images, new_images);

        // Iterate over the updated images and ensure that they only target A/B volume pairs or ESP
        // partitions. If an image targets a standalone block device, return an error.
        for (device_id, _) in updated_images {
            // Get lists of ESP images and A/B volume pair images
            let esp_images = ctx.spec.storage.get_esp_images();
            let ab_volume_pair_images = ctx.spec.storage.get_ab_volume_pair_images();

            // If the device ID is not found in the list of ESP images or A/B volume pair images, return
            // an error
            if !esp_images.iter().any(|(id, _)| id == &device_id)
                && !ab_volume_pair_images.iter().any(|(id, _)| id == &device_id)
            {
                return Err(TridentError::new(InvalidInputError::from(
                    HostConfigurationDynamicValidationError::ImageUpdateOnStandaloneBlockDevice {
                        device_id: device_id.clone(),
                    },
                )));
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    use maplit::btreemap;
    use url::Url;
    use uuid::Uuid;

    use osutils::{
        arch::SystemArchitecture, osrelease::OsRelease, osuuid::OsUuid,
        partition_types::DiscoverablePartitionType,
    };

    use trident_api::{
        config::{
            AbUpdate, AbVolumePair, Disk, FileSystem, FileSystemSource, FileSystemType,
            HostConfiguration, ImageSha384, MountOptions, MountPoint, OsImage as OsImageConfig,
            Partition, PartitionType, Storage as StorageConfig,
        },
        status::{AbVolumeSelection, ServicingType},
    };

    use crate::osimage::{
        mock::{MockImage, MockOsImage},
        OsImage,
    };

    const OSIMAGE_DUMMY_SOURCE: &str = "http://example/osimage";

    /// Validates that the logic in ab_update_required() is correct when OS image is used.
    #[test]
    fn test_ab_update_required_os_image() {
        // Initialize a Host Configuration with COSI enabled and OS image provided
        let hc_os_image = HostConfiguration {
            image: Some(OsImageConfig {
                url: Url::parse("http://example.com/osimage").unwrap(),
                sha384: ImageSha384::Ignored,
            }),
            storage: StorageConfig {
                disks: vec![Disk {
                    id: "os".to_owned(),
                    device: PathBuf::from("/dev/disk/by-bus/foobar"),
                    partitions: vec![
                        Partition {
                            id: "esp".to_string(),
                            partition_type: PartitionType::Esp,
                            size: 100.into(),
                        },
                        Partition {
                            id: "root-a".to_string(),
                            partition_type: PartitionType::Root,
                            size: 100.into(),
                        },
                        Partition {
                            id: "root-b".to_string(),
                            partition_type: PartitionType::Root,
                            size: 100.into(),
                        },
                        Partition {
                            id: "trident".to_string(),
                            partition_type: PartitionType::LinuxGeneric,
                            size: 100.into(),
                        },
                    ],
                    ..Default::default()
                }],
                filesystems: vec![
                    FileSystem {
                        device_id: Some("esp".into()),
                        fs_type: FileSystemType::Vfat,
                        source: FileSystemSource::OsImage,
                        mount_point: Some(MountPoint {
                            path: PathBuf::from("/boot/efi"),
                            options: MountOptions::empty(),
                        }),
                    },
                    FileSystem {
                        device_id: Some("root".into()),
                        fs_type: FileSystemType::Vfat,
                        source: FileSystemSource::OsImage,
                        mount_point: Some(MountPoint {
                            path: PathBuf::from("/"),
                            options: MountOptions::empty(),
                        }),
                    },
                    FileSystem {
                        device_id: Some("trident".into()),
                        fs_type: FileSystemType::Vfat,
                        source: FileSystemSource::OsImage,
                        mount_point: None,
                    },
                ],
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
        };

        // Initialize an engine context object with spec matching the Host Configuration
        // Generate mock OS image
        let mock_entries = [
            ("/image/path/A", "ext4", FileSystemType::Ext4),
            ("/image/path/B", "ext4", FileSystemType::Ext4),
        ]
        .into_iter();

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

        // Test case #1: If OS image has not changed, return false.
        let mut ctx = EngineContext {
            servicing_type: ServicingType::NoActiveServicing,
            spec: hc_os_image.clone(),
            spec_old: hc_os_image.clone(),
            partition_paths: btreemap! {
                "os".into() => PathBuf::from("/dev/disk/by-bus/foobar"),
                "esp".into() => PathBuf::from("/dev/disk/by-partlabel/osp1"),
                "root-a".into() => PathBuf::from("/dev/disk/by-partlabel/osp2"),
                "root-b".into() => PathBuf::from("/dev/disk/by-partlabel/osp3"),
                "trident".into() => PathBuf::from("/dev/disk/by-partlabel/osp4"),
            },
            // Set active volume to A
            ab_active_volume: Some(AbVolumeSelection::VolumeA),
            image: Some(os_image),
            ..Default::default()
        };
        assert!(!ab_update_required(&ctx).unwrap());

        // Test case #2: If OS image has changed, return true.
        let mut hc_os_image_updated = hc_os_image.clone();
        // Update OS image URL
        hc_os_image_updated.image = Some(OsImageConfig {
            url: Url::parse("http://example.com/osimage_2").unwrap(),
            sha384: ImageSha384::Ignored,
        });
        ctx.spec = hc_os_image_updated;
        assert!(ab_update_required(&ctx).unwrap());
    }
}
