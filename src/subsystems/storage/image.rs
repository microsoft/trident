use log::debug;

use trident_api::{
    config::{HostConfigurationDynamicValidationError, ImageSha384},
    error::{InvalidInputError, TridentError},
};

use crate::engine::EngineContext;

/// Checks if the host needs an A/B update. First, compares the partition images in the specs. If
/// the partition images have not been updated, checks if the new Host Configuration requests an OS
/// image. If yes, update is needed, unless the old Host Configuration also requested an OS image
/// and the URLs and SHA256 checksums are the same.
///
/// TODO: Remove this logic for partition images once COSI becomes the default for GA.
pub(super) fn ab_update_required(ctx: &EngineContext) -> Result<bool, TridentError> {
    debug!("Checking OS image to determine if an A/B update is required");
    // Otherwise, continue checking OS images
    match (&ctx.spec_old.image, &ctx.spec.image) {
        // If OS image is not requested in the new spec, no update is needed.
        (None, None) => Ok(false),

        (None, Some(_)) => {
            // This is most likely an offline-init's first update.
            Ok(true)
        }

        // Update if the sha384 has changed (including if one is 'ignored'), or both are ignored but
        // the URL has changed.
        (Some(old_os_image), Some(new_os_image)) => Ok(old_os_image.sha384 != new_os_image.sha384
            || old_os_image.sha384 == ImageSha384::Ignored && old_os_image.url != new_os_image.url),

        (Some(_), None) => {
            // Return an error if the old spec requests an OS image but the new spec does not.
            Err(TridentError::new(InvalidInputError::from(
                HostConfigurationDynamicValidationError::DeployPartitionImagesAfterOsImage,
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    use maplit::btreemap;
    use url::Url;
    use uuid::Uuid;

    use osutils::osrelease::OsRelease;
    use sysdefs::{
        arch::SystemArchitecture, osuuid::OsUuid, partition_types::DiscoverablePartitionType,
    };

    use trident_api::{
        config::{
            AbUpdate, AbVolumePair, Disk, FileSystem, FileSystemSource, HostConfiguration,
            ImageSha384, MountOptions, MountPoint, OsImage as OsImageConfig, Partition,
            PartitionType, Storage as StorageConfig,
        },
        status::{AbVolumeSelection, ServicingType},
    };

    use crate::osimage::{
        mock::{MockImage, MockOsImage},
        OsImage, OsImageFileSystemType,
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
                        source: FileSystemSource::Image,
                        mount_point: Some(MountPoint {
                            path: PathBuf::from("/boot/efi"),
                            options: MountOptions::empty(),
                        }),
                    },
                    FileSystem {
                        device_id: Some("root".into()),
                        source: FileSystemSource::Image,
                        mount_point: Some(MountPoint {
                            path: PathBuf::from("/"),
                            options: MountOptions::empty(),
                        }),
                    },
                    FileSystem {
                        device_id: Some("trident".into()),
                        source: FileSystemSource::Image,
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
            ("/image/path/A", OsImageFileSystemType::Ext4),
            ("/image/path/B", OsImageFileSystemType::Ext4),
        ]
        .into_iter();

        let os_image = OsImage::mock(MockOsImage {
            source: Url::parse(OSIMAGE_DUMMY_SOURCE).unwrap(),
            os_arch: SystemArchitecture::Amd64,
            os_release: OsRelease::default(),
            is_uki: false,
            images: mock_entries
                .clone()
                .map(|(path, fs_type)| MockImage {
                    mount_point: PathBuf::from(path),
                    fs_type,
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
