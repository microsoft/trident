use std::path::Path;

use serde::{Deserialize, Serialize};

#[cfg(feature = "schemars")]
use schemars::JsonSchema;

use crate::{
    constants::{self, ROOT_MOUNT_POINT_PATH, TRIDENT_OVERLAY_PATH},
    is_default, BlockDeviceId,
};

use super::error::InvalidHostConfigurationError;

pub mod blkdev_graph;
pub mod disks;
pub mod encryption;
pub mod imaging;
pub mod mountpoint;
pub mod partitions;
pub mod raid;
mod serde_hash;
pub mod verity;

use self::{
    blkdev_graph::{
        builder::BlockDeviceGraphBuilder, error::BlockDeviceGraphBuildError,
        graph::BlockDeviceGraph,
    },
    disks::Disk,
    encryption::Encryption,
    imaging::{AbUpdate, Image},
    mountpoint::MountPoint,
    partitions::Partition,
    raid::Raid,
    verity::VerityDevice,
};

/// Storage configuration describes the disks of the host that will be used to
/// store the OS and data. Not all disks of the host need to be captured inside
/// the Host Configuration, only those that Trident should operate on.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct Storage {
    /// A list of disks that will be used for the host.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disks: Vec<Disk>,

    /// Encryption configuration.
    #[serde(default, skip_serializing_if = "is_default")]
    pub encryption: Option<Encryption>,

    /// RAID configuration.
    #[serde(default, skip_serializing_if = "is_default")]
    pub raid: Raid,

    /// Verity configuration.
    #[serde(default, skip_serializing_if = "is_default")]
    pub verity: Vec<VerityDevice>,

    /// Mount point configuration.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mount_points: Vec<MountPoint>,

    /// A/B update configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ab_update: Option<AbUpdate>,

    /// A list of images to be written to the host.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<Image>,
}

impl Storage {
    pub fn get_partition(&self, id: &BlockDeviceId) -> Option<&Partition> {
        self.disks
            .iter()
            .flat_map(|d| d.partitions.iter())
            .find(|p| &p.id == id)
    }

    pub fn build_graph(&self) -> Result<BlockDeviceGraph<'_>, BlockDeviceGraphBuildError> {
        let mut builder = BlockDeviceGraphBuilder::default();

        // Add disks
        for disk in &self.disks {
            builder.add_node(disk.into());
            for partition in &disk.partitions {
                builder.add_node(partition.into());
            }
        }

        // Add RAID arrays
        for raid in &self.raid.software {
            builder.add_node(raid.into());
        }

        // Add A/B update volume pairs
        if let Some(ab_update) = &self.ab_update {
            for pair in &ab_update.volume_pairs {
                builder.add_node(pair.into());
            }
        }

        // Add encrypted volumes
        if let Some(encryption) = &self.encryption {
            for volume in &encryption.volumes {
                builder.add_node(volume.into());
            }
        }

        // Add verity devices
        for verity in &self.verity {
            builder.add_node(verity.into());
        }

        // Add mount points
        for mount_point in &self.mount_points {
            builder.add_mount_point(mount_point);
        }

        // Add images
        for image in &self.images {
            builder.add_image(image);
        }

        // Try to build the graph
        builder.build()
    }

    /// Validate the storage configuration
    ///
    /// This function will validate the storage configuration and return an error
    /// if the configuration is invalid.
    pub fn validate(
        &self,
        require_root_mount_point: bool,
    ) -> Result<(), InvalidHostConfigurationError> {
        // Check basic constraints

        if let Some(encryption) = &self.encryption {
            encryption.validate()?;
        }

        // Build the graph
        let graph = self.build_graph()?;

        // If storage configuration is requested, then ESP volume must be
        // present, to update Grub configuration
        if *self != Storage::default() {
            graph.validate_volume_presence(Path::new(constants::ESP_MOUNT_POINT_PATH))?;
        }
        // If either storage configuration is requested or other modules require
        // root mount point, ensure the root mount point is present
        if require_root_mount_point || *self != Storage::default() {
            graph.validate_volume_presence(Path::new(constants::ROOT_MOUNT_POINT_PATH))?;
        }

        if !self.verity.is_empty() {
            // Depends on root mount point validated above
            self.validate_verity(&graph)?;
        }

        Ok(())
    }

    /// Validates the verity configuration. Assumes the verity list of devices
    /// is not empty.
    fn validate_verity(
        &self,
        graph: &BlockDeviceGraph,
    ) -> Result<(), InvalidHostConfigurationError> {
        if self.verity.is_empty() {
            panic!("validate_verity() called with empty verity configuration");
        }

        // Verity is only supported for root volume, verify the input is not
        // asking for something else
        if self.verity.len() > 1 {
            return Err(InvalidHostConfigurationError::UnsupportedVerityDevices);
        }

        let verity_device = &self.verity[0];

        let root_mount_point = &self
            .mount_points
            .iter()
            .find(|mp| mp.path == Path::new(ROOT_MOUNT_POINT_PATH));
        if root_mount_point.is_none() {
            return Err(InvalidHostConfigurationError::ExpectedMountPointNotFound {
                mount_point_path: ROOT_MOUNT_POINT_PATH.into(),
            });
        }
        let root_mount_point = root_mount_point.unwrap();

        if root_mount_point.target_id != verity_device.id {
            return Err(InvalidHostConfigurationError::UnsupportedVerityDevices);
        }

        // If root verity is required, we also require dedicated /boot
        // partition, as we otherwise cannot modify grub configuration and
        // kernel command line.
        graph.validate_volume_presence(Path::new(constants::BOOT_MOUNT_POINT_PATH))?;

        // For root verity, we also require an overlay for /etc, so that we can
        // inject configuration generated by Trident. This overlay needs to be
        // stored on a separate partition, as the root partition is read-only.
        // For the initial release, we are not exposing configuration of this
        // overlay backing partition to user, but instead, we will expect
        // /var/lib/trident-overlay to be present and use it as the backing
        // partition for the overlay. /var/lib/trident-overlay needs to be
        // backed by an A/B update volume pair and not reside on a read-only
        // volume.
        let overlay_support_mount_point = self
            .path_to_mount_point(Path::new(TRIDENT_OVERLAY_PATH))
            .ok_or(InvalidHostConfigurationError::ExpectedMountPointNotFound {
                mount_point_path: ROOT_MOUNT_POINT_PATH.into(),
            })?;
        let overlay_block_device_id = &overlay_support_mount_point.target_id;

        // If some ab_update is present, the overlay must be also on an ab
        // volume.
        if let Some(ab_update) = &self.ab_update {
            if !ab_update
                .volume_pairs
                .iter()
                .any(|p| p.id == *overlay_block_device_id)
            {
                return Err(
                    InvalidHostConfigurationError::MountPointNotBackedByAbUpdateVolumePair {
                        mount_point_path: TRIDENT_OVERLAY_PATH.into(),
                    },
                );
            }
        }

        // Ensure the overlay is not on a read-only volume
        if overlay_support_mount_point
            .options
            .contains(&"ro".to_string())
        {
            return Err(InvalidHostConfigurationError::OverlayOnReadOnlyVolume {
                mount_point_path: overlay_support_mount_point
                    .path
                    .to_string_lossy()
                    .to_string(),
                overlay_path: TRIDENT_OVERLAY_PATH.into(),
            });
        }

        // Ensure the overlay is not on a verity protected volume
        if self.verity.iter().any(|v| v.id == *overlay_block_device_id) {
            return Err(InvalidHostConfigurationError::OverlayOnReadOnlyVolume {
                mount_point_path: overlay_support_mount_point
                    .path
                    .to_string_lossy()
                    .to_string(),
                overlay_path: TRIDENT_OVERLAY_PATH.into(),
            });
        }

        // Ensure the root verity device name is set to `root`, as that is what
        // the dracut verity module expects.
        if verity_device.device_name != "root" {
            return Err(InvalidHostConfigurationError::RootVerityDeviceNameInvalid {
                device_name: verity_device.device_name.clone(),
            });
        }

        // Ensure the root verity device is mounted read-only at /.
        if !root_mount_point.options.contains(&"ro".to_owned()) {
            return Err(InvalidHostConfigurationError::VerityDeviceReadWrite {
                device_name: verity_device.device_name.clone(),
                mount_point_path: root_mount_point.path.to_string_lossy().to_string(),
            });
        }

        // Ensure the root verity device is not mounted read-write anywhere.
        if let Some(mp) = self
            .mount_points
            .iter()
            .find(|mp| mp.target_id == verity_device.id && !mp.options.contains(&"ro".to_owned()))
        {
            return Err(InvalidHostConfigurationError::VerityDeviceReadWrite {
                device_name: verity_device.device_name.clone(),
                mount_point_path: mp.path.to_string_lossy().to_string(),
            });
        }

        Ok(())
    }

    /// Find the mount point that is holding the given path. This is useful to find
    /// the volume on which the given absolute path is located. This version uses HC
    /// to find the information and is useful early in the process when HS has not
    /// yet been populated.
    pub fn path_to_mount_point<'a>(&'a self, path: &Path) -> Option<&'a MountPoint> {
        self.mount_points
            .iter()
            .filter(|mp| path.starts_with(&mp.path))
            .max_by_key(|mp| mp.path.as_os_str().len())
    }
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, str::FromStr};

    use url::Url;

    use crate::{
        config::{
            host::storage::blkdev_graph::types::{BlkDevKind, BlkDevReferrerKind},
            HostConfiguration,
        },
        constants::ROOT_MOUNT_POINT_PATH,
    };

    use self::{
        disks::PartitionTableType,
        encryption::EncryptedVolume,
        imaging::{AbVolumePair, ImageFormat, ImageSha256},
        partitions::{PartitionSize, PartitionType},
        raid::{RaidLevel, SoftwareRaidArray},
    };

    use super::*;

    /// Generate a basic valid Storage configuration for testing.
    fn get_storage() -> Storage {
        Storage {
            disks: vec![
                Disk {
                    id: "disk1".to_owned(),
                    device: constants::ROOT_MOUNT_POINT_PATH.into(),
                    ..Default::default()
                },
                Disk {
                    id: "disk2".to_owned(),
                    device: "/etc".into(),
                    partitions: vec![
                        Partition {
                            id: "esp".to_owned(),
                            partition_type: PartitionType::Esp,
                            size: PartitionSize::from_str("1M").unwrap(),
                        },
                        Partition {
                            id: "root-a".to_owned(),
                            partition_type: PartitionType::Root,
                            size: PartitionSize::from_str("1G").unwrap(),
                        },
                        Partition {
                            id: "root-a-verity".to_owned(),
                            partition_type: PartitionType::RootVerity,
                            size: PartitionSize::from_str("1G").unwrap(),
                        },
                        Partition {
                            id: "root-b".to_owned(),
                            partition_type: PartitionType::Root,
                            size: PartitionSize::from_str("1G").unwrap(),
                        },
                        Partition {
                            id: "root-b-verity".to_owned(),
                            partition_type: PartitionType::RootVerity,
                            size: PartitionSize::from_str("1G").unwrap(),
                        },
                        Partition {
                            id: "mnt-raid-1".to_owned(),
                            partition_type: PartitionType::LinuxGeneric,
                            size: PartitionSize::from_str("1G").unwrap(),
                        },
                        Partition {
                            id: "mnt-raid-2".to_owned(),
                            partition_type: PartitionType::LinuxGeneric,
                            size: PartitionSize::from_str("1G").unwrap(),
                        },
                        Partition {
                            id: "srv-enc".to_owned(),
                            partition_type: PartitionType::LinuxGeneric,
                            size: PartitionSize::from_str("1G").unwrap(),
                        },
                        Partition {
                            id: "boot".to_owned(),
                            partition_type: PartitionType::LinuxGeneric,
                            size: PartitionSize::from_str("1G").unwrap(),
                        },
                        Partition {
                            id: "overlay".to_owned(),
                            partition_type: PartitionType::LinuxGeneric,
                            size: PartitionSize::from_str("1G").unwrap(),
                        },
                    ],
                    ..Default::default()
                },
            ],
            encryption: Some(Encryption {
                recovery_key_url: Some(Url::parse("file:///recovery.key").unwrap()),
                volumes: vec![EncryptedVolume {
                    id: "srv".to_owned(),
                    device_name: "luks-srv".to_owned(),
                    target_id: "srv-enc".to_owned(),
                }],
            }),
            raid: Raid {
                software: vec![SoftwareRaidArray {
                    id: "mnt".to_owned(),
                    name: "md-mnt".to_owned(),
                    level: RaidLevel::Raid1,
                    metadata_version: "1.2".to_owned(),
                    devices: vec!["mnt-raid-1".to_owned(), "mnt-raid-2".to_owned()],
                }],
            },
            verity: vec![],
            mount_points: vec![
                MountPoint {
                    path: PathBuf::from(constants::ROOT_MOUNT_POINT_PATH),
                    filesystem: "ext4".to_owned(),
                    options: Vec::new(),
                    target_id: "root".to_owned(),
                },
                MountPoint {
                    path: PathBuf::from("/boot/efi"),
                    filesystem: "vfat".to_owned(),
                    options: Vec::new(),
                    target_id: "esp".to_owned(),
                },
                MountPoint {
                    path: PathBuf::from("/mnt"),
                    filesystem: "ext4".to_owned(),
                    options: Vec::new(),
                    target_id: "mnt".to_owned(),
                },
                MountPoint {
                    path: PathBuf::from("/srv"),
                    filesystem: "ext4".to_owned(),
                    options: Vec::new(),
                    target_id: "srv".to_owned(),
                },
                MountPoint {
                    path: PathBuf::from("/boot"),
                    filesystem: "ext4".to_owned(),
                    options: Vec::new(),
                    target_id: "boot".to_owned(),
                },
                MountPoint {
                    path: PathBuf::from(TRIDENT_OVERLAY_PATH),
                    filesystem: "ext4".to_owned(),
                    options: Vec::new(),
                    target_id: "overlay".to_owned(),
                },
            ],
            images: vec![
                Image {
                    target_id: "esp".to_owned(),
                    url: "file:///esp.raw.zst".to_owned(),
                    sha256: ImageSha256::Ignored,
                    format: ImageFormat::RawZst,
                },
                Image {
                    target_id: "root".to_owned(),
                    url: "file:///root.raw.zst".to_owned(),
                    sha256: ImageSha256::Ignored,
                    format: ImageFormat::RawZst,
                },
                Image {
                    target_id: "root-a-verity".to_owned(),
                    url: "file:///root-hash.raw.zst".to_owned(),
                    sha256: ImageSha256::Ignored,
                    format: ImageFormat::RawZst,
                },
                Image {
                    target_id: "boot".to_owned(),
                    url: "file:///boot.raw.zst".to_owned(),
                    sha256: ImageSha256::Ignored,
                    format: ImageFormat::RawZst,
                },
            ],
            ab_update: Some(AbUpdate {
                volume_pairs: vec![AbVolumePair {
                    id: "root".to_owned(),
                    volume_a_id: "root-a".to_owned(),
                    volume_b_id: "root-b".to_owned(),
                }],
            }),
        }
    }

    /// Test that validates that to_sdrepart_part_type() returns the correct string for each
    /// PartitionType.
    #[test]
    fn test_to_sdrepart_part_type() {
        assert_eq!(PartitionType::Esp.to_sdrepart_part_type(), "esp");
        assert_eq!(PartitionType::Home.to_sdrepart_part_type(), "home");
        assert_eq!(
            PartitionType::LinuxGeneric.to_sdrepart_part_type(),
            "linux-generic"
        );
        assert_eq!(PartitionType::Root.to_sdrepart_part_type(), "root");
        assert_eq!(
            PartitionType::RootVerity.to_sdrepart_part_type(),
            "root-verity"
        );
        assert_eq!(PartitionType::Swap.to_sdrepart_part_type(), "swap");
        assert_eq!(PartitionType::Tmp.to_sdrepart_part_type(), "tmp");
        assert_eq!(PartitionType::Usr.to_sdrepart_part_type(), "usr");
        assert_eq!(PartitionType::Var.to_sdrepart_part_type(), "var");
    }

    #[test]
    fn test_get_partition() {
        let storage = Storage {
            disks: vec![
                Disk {
                    id: "disk1".to_string(),
                    device: PathBuf::from("/dev/sda"),
                    partition_table_type: PartitionTableType::Gpt,
                    partitions: vec![
                        Partition {
                            id: "disk1-partition1".to_string(),
                            partition_type: PartitionType::Esp,
                            size: PartitionSize::from_str("1M").unwrap(),
                        },
                        Partition {
                            id: "disk1-partition2".to_string(),
                            partition_type: PartitionType::Root,
                            size: PartitionSize::from_str("1G").unwrap(),
                        },
                    ],
                    ..Default::default()
                },
                Disk {
                    id: "disk2".to_string(),
                    device: PathBuf::from("/dev/sdb"),
                    partition_table_type: PartitionTableType::Gpt,
                    partitions: vec![Partition {
                        id: "disk2-partition1".to_string(),
                        partition_type: PartitionType::Esp,
                        size: PartitionSize::from_str("1M").unwrap(),
                    }],
                    ..Default::default()
                },
            ],
            ..Default::default()
        };

        let partition = storage
            .get_partition(&"disk1-partition1".to_string())
            .expect("Expected to find a partition but not found.");

        assert_eq!(partition.id, "disk1-partition1");
        assert_eq!(partition.partition_type, crate::config::PartitionType::Esp);
        assert_eq!(partition.size, crate::config::PartitionSize::Fixed(1048576));

        let partition = storage.get_partition(&"non_existing_partition".to_string());
        assert_eq!(partition, None);
    }

    #[test]
    fn test_validate() {
        let storage = Storage {
            disks: vec![
                Disk {
                    id: "disk1".to_string(),
                    device: PathBuf::from("/dev/sda"),
                    partition_table_type: PartitionTableType::Gpt,
                    partitions: vec![
                        Partition {
                            id: "disk1-partition1".to_string(),
                            partition_type: PartitionType::Esp,
                            size: PartitionSize::from_str("1M").unwrap(),
                        },
                        Partition {
                            id: "disk1-partition2".to_string(),
                            partition_type: PartitionType::Root,
                            size: PartitionSize::from_str("1G").unwrap(),
                        },
                    ],
                    ..Default::default()
                },
                Disk {
                    id: "disk2".to_string(),
                    device: PathBuf::from("/dev/sdb"),
                    partition_table_type: PartitionTableType::Gpt,
                    partitions: vec![
                        Partition {
                            id: "disk2-partition1".to_string(),
                            partition_type: PartitionType::Esp,
                            size: PartitionSize::from_str("1M").unwrap(),
                        },
                        Partition {
                            id: "disk2-partition2".to_string(),
                            partition_type: PartitionType::Root,
                            size: PartitionSize::from_str("1G").unwrap(),
                        },
                    ],
                    ..Default::default()
                },
            ],
            mount_points: vec![
                MountPoint {
                    filesystem: "ext4".to_string(),
                    options: vec![],
                    target_id: "disk1-partition2".to_string(),
                    path: PathBuf::from(constants::ROOT_MOUNT_POINT_PATH),
                },
                MountPoint {
                    filesystem: "ext4".to_string(),
                    options: vec![],
                    target_id: "disk1-partition1".to_string(),
                    path: PathBuf::from("/boot/efi"),
                },
            ],
            images: vec![
                Image {
                    format: imaging::ImageFormat::RawZst,
                    target_id: "disk1-partition2".to_string(),
                    url: "http://example.com/image".to_string(),
                    sha256: imaging::ImageSha256::Ignored,
                },
                Image {
                    format: imaging::ImageFormat::RawZst,
                    target_id: "disk1-partition1".to_string(),
                    url: "http://example.com/image".to_string(),
                    sha256: imaging::ImageSha256::Ignored,
                },
            ],
            ..Default::default()
        };
        storage.validate(true).unwrap();

        let mount_volume_pair = Storage {
            ab_update: Some(AbUpdate {
                volume_pairs: vec![imaging::AbVolumePair {
                    id: "ab-update-volume-pair".to_string(),
                    volume_a_id: "disk1-partition2".to_string(),
                    volume_b_id: "disk2-partition2".to_string(),
                }],
            }),
            mount_points: vec![
                MountPoint {
                    filesystem: "ext4".to_string(),
                    options: vec![],
                    target_id: "ab-update-volume-pair".to_string(),
                    path: PathBuf::from(constants::ROOT_MOUNT_POINT_PATH),
                },
                MountPoint {
                    filesystem: "ext4".to_string(),
                    options: vec![],
                    target_id: "disk1-partition1".to_string(),
                    path: PathBuf::from("/boot/efi"),
                },
            ],
            images: vec![
                Image {
                    format: imaging::ImageFormat::RawZst,
                    target_id: "ab-update-volume-pair".to_string(),
                    url: "http://example.com/image".to_string(),
                    sha256: imaging::ImageSha256::Ignored,
                },
                Image {
                    format: imaging::ImageFormat::RawZst,
                    target_id: "disk1-partition1".to_string(),
                    url: "http://example.com/image".to_string(),
                    sha256: imaging::ImageSha256::Ignored,
                },
            ],
            ..storage.clone()
        };
        mount_volume_pair.validate(true).unwrap();

        let bad_volume_pair = Storage {
            ab_update: Some(AbUpdate {
                volume_pairs: vec![imaging::AbVolumePair {
                    id: "ab-update-volume-pair".to_string(),
                    volume_a_id: "disk1-partition1".to_string(),
                    volume_b_id: "disk1-partition1".to_string(),
                }],
            }),
            ..storage.clone()
        };
        assert_eq!(
            bad_volume_pair.validate(true).unwrap_err(),
            InvalidHostConfigurationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::DuplicateTargetId {
                    node_id: "ab-update-volume-pair".into(),
                    kind: BlkDevKind::ABVolume,
                    target_id: "disk1-partition1".into()
                }
            )
        );

        let bad_volume_pair_id = Storage {
            ab_update: Some(AbUpdate {
                volume_pairs: vec![imaging::AbVolumePair {
                    id: "disk1".to_string(),
                    volume_a_id: "disk1-partition2".to_string(),
                    volume_b_id: "disk2-partition2".to_string(),
                }],
            }),
            ..storage.clone()
        };
        assert_eq!(
            bad_volume_pair_id.validate(true).unwrap_err(),
            InvalidHostConfigurationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::DuplicateDeviceId("disk1".into())
            )
        );

        let bad_image_target = Storage {
            images: vec![Image {
                format: imaging::ImageFormat::RawZst,
                target_id: "disk99".to_string(),
                url: "http://example.com/image".to_string(),
                sha256: imaging::ImageSha256::Ignored,
            }],
            ..storage.clone()
        };
        assert_eq!(
            bad_image_target.validate(true).unwrap_err(),
            InvalidHostConfigurationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::ImageNonExistentReference {
                    image_id: "http://example.com/image".into(),
                    target_id: "disk99".into()
                }
            )
        );
    }

    #[test]
    fn test_validate2() {
        Storage::default().validate(false).unwrap();

        let mut storage = Storage {
            disks: vec![
                Disk {
                    id: "disk1".to_owned(),
                    device: constants::ROOT_MOUNT_POINT_PATH.into(),
                    ..Default::default()
                },
                Disk {
                    id: "disk2".to_owned(),
                    device: "/tmp".into(),
                    partitions: vec![
                        Partition {
                            id: "part1".to_owned(),
                            partition_type: PartitionType::Esp,
                            size: PartitionSize::from_str("1M").unwrap(),
                        },
                        Partition {
                            id: "part2".to_owned(),
                            partition_type: PartitionType::Root,
                            size: PartitionSize::from_str("1G").unwrap(),
                        },
                        Partition {
                            id: "part3".to_owned(),
                            partition_type: PartitionType::Root,
                            size: PartitionSize::from_str("1G").unwrap(),
                        },
                        Partition {
                            id: "part4".to_owned(),
                            partition_type: PartitionType::Root,
                            size: PartitionSize::from_str("1G").unwrap(),
                        },
                        Partition {
                            id: "part5".to_owned(),
                            partition_type: PartitionType::Root,
                            size: PartitionSize::from_str("1G").unwrap(),
                        },
                    ],
                    ..Default::default()
                },
            ],
            raid: Raid {
                software: vec![SoftwareRaidArray {
                    id: "my-raid1".to_owned(),
                    name: "my-raid".to_owned(),
                    level: RaidLevel::Raid1,
                    metadata_version: "1.2".to_owned(),
                    devices: vec!["part3".to_owned(), "part4".to_owned()],
                }],
            },
            mount_points: vec![
                MountPoint {
                    filesystem: "ext4".to_owned(),
                    options: vec![],
                    target_id: "ab1".to_owned(),
                    path: PathBuf::from(constants::ROOT_MOUNT_POINT_PATH),
                },
                MountPoint {
                    filesystem: "ext4".to_owned(),
                    options: vec![],
                    target_id: "part1".to_owned(),
                    path: PathBuf::from("/boot/efi"),
                },
            ],
            images: vec![
                Image {
                    target_id: "ab1".to_owned(),
                    url: "https://some/url".to_owned(),
                    sha256: imaging::ImageSha256::Checksum("".into()),
                    format: ImageFormat::RawZst,
                },
                Image {
                    target_id: "part1".to_owned(),
                    url: "https://some/url".to_owned(),
                    sha256: imaging::ImageSha256::Checksum("".into()),
                    format: ImageFormat::RawZst,
                },
            ],
            ab_update: Some(AbUpdate {
                volume_pairs: vec![AbVolumePair {
                    id: "ab1".to_owned(),
                    volume_a_id: "part5".to_owned(),
                    volume_b_id: "part2".to_owned(),
                }],
            }),
            encryption: None,
            verity: vec![],
        };
        storage.validate(true).unwrap();

        let storage_golden = storage.clone();

        // fail on duplicate id
        storage = storage_golden.clone();
        storage.disks.get_mut(0).unwrap().partitions = vec![Partition {
            id: "part1".to_owned(),
            partition_type: PartitionType::Esp,
            size: PartitionSize::from_str("1M").unwrap(),
        }];
        assert_eq!(
            storage.validate(true).unwrap_err(),
            InvalidHostConfigurationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::DuplicateDeviceId("part1".into())
            ),
        );

        // fail on duplicate id
        storage = storage_golden.clone();
        storage.ab_update.as_mut().unwrap().volume_pairs[0].id = "disk1".to_owned();
        assert_eq!(
            storage.validate(true).unwrap_err(),
            InvalidHostConfigurationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::DuplicateDeviceId("disk1".into())
            ),
        );

        // fail on missing reference (disk4 does not exist)
        storage = storage_golden.clone();
        storage.ab_update.as_mut().unwrap().volume_pairs[0].volume_a_id = "disk4".to_owned();
        assert_eq!(
            storage.validate(true).unwrap_err(),
            InvalidHostConfigurationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::NonExistentReference {
                    node_id: "ab1".into(),
                    kind: BlkDevKind::ABVolume,
                    target_id: "disk4".into()
                }
            ),
        );

        // fail on missing reference (disk4 does not exist)
        storage = storage_golden.clone();
        storage.images[0].target_id = "disk4".to_owned();
        assert_eq!(
            storage.validate(true).unwrap_err(),
            InvalidHostConfigurationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::ImageNonExistentReference {
                    image_id: "https://some/url".into(),
                    target_id: "disk4".into()
                }
            ),
        );

        // fail on missing reference (disk4 does not exist)
        storage = storage_golden.clone();
        storage.mount_points[0].target_id = "disk4".to_owned();
        assert_eq!(
            storage.validate(true).unwrap_err(),
            InvalidHostConfigurationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::MountPointNonExistentReference {
                    mount_point: "/".into(),
                    target_id: "disk4".into()
                }
            ),
        );

        // fail on bad block device type
        storage = storage_golden.clone();
        storage.images[0].target_id = "disk1".to_owned();
        assert_eq!(
            storage.validate(true).unwrap_err(),
            InvalidHostConfigurationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::ImageInvalidReference {
                    image_id: "https://some/url".into(),
                    target_id: "disk1".into(),
                    target_kind: BlkDevKind::Disk,
                    valid_references: BlkDevReferrerKind::Image.valid_target_kinds()
                }
            ),
        );

        // fail if devices are not all the same size for a RAID
        storage = storage_golden.clone();
        storage.disks[1].partitions[3].size = PartitionSize::from_str("2G").unwrap();
        assert_eq!(
            storage.validate(true).unwrap_err(),
            InvalidHostConfigurationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::InvalidTargets {
                    node_id: "my-raid1".into(),
                    kind: BlkDevKind::RaidArray,
                    body: "RAID array references partitions with different sizes.".into()
                }
            ),
        );
    }

    #[test]
    fn test_validate_encryption_pass() {
        let storage: Storage = get_storage();
        storage.validate(true).unwrap();
    }

    /// A/B update volume pairs can target encrypted volumes (A)
    #[test]
    fn test_validate_ab_update_volume_pair_a_id_encryption_pass() {
        let mut storage: Storage = get_storage();
        storage.ab_update.as_mut().unwrap().volume_pairs[0].volume_a_id = "srv".to_owned();
        // Delete mount point associated with "srv", otherwise this would fail
        storage.mount_points.retain(|mp| mp.target_id != "srv");
        storage.validate(true).unwrap();
    }

    /// A/B update volume pairs can target encrypted volumes (B)
    #[test]
    fn test_validate_ab_update_volume_pair_b_id_encryption_pass() {
        let mut storage: Storage = get_storage();
        storage.ab_update.as_mut().unwrap().volume_pairs[0].volume_b_id = "srv".to_owned();
        // Delete mount point associated with "srv", otherwise this would fail
        storage.mount_points.retain(|mp| mp.target_id != "srv");
        storage.validate(true).unwrap();
    }

    /// Software RAID arrays must have one or more devices
    #[test]
    fn test_validate_software_raid_array_no_devices_fail() {
        let mut storage: Storage = get_storage();
        storage.raid.software[0].devices = Vec::new();
        assert_eq!(
            storage.validate(true).unwrap_err(),
            InvalidHostConfigurationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::InvalidTargetCount {
                    node_id: "mnt".into(),
                    kind: BlkDevKind::RaidArray,
                    target_count: 0,
                    expected: BlkDevReferrerKind::RaidArray.valid_target_count()
                }
            )
        );
    }

    /// Software RAID arrays cannot target encrypted volumes
    #[test]
    fn test_validate_software_raid_target_id_encryption_fail() {
        let mut storage: Storage = get_storage();
        storage.raid.software[0].devices[0] = "srv".to_owned();
        eprintln!("{:?}", storage.validate(true).unwrap_err());
        assert_eq!(
            storage.validate(true).unwrap_err(),
            InvalidHostConfigurationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::InvalidReferenceKind {
                    node_id: "mnt".into(),
                    kind: BlkDevKind::RaidArray,
                    target_id: "srv".into(),
                    target_kind: BlkDevKind::EncryptedVolume,
                    valid_references: BlkDevReferrerKind::RaidArray.valid_target_kinds()
                }
            ),
        );
    }

    /// Encrypted volumes and disks must not share the same id
    #[test]
    fn test_validate_encryption_disks_share_id_fail() {
        let mut storage: Storage = get_storage();
        storage.encryption.as_mut().unwrap().volumes[0].id = "disk1".to_owned();
        assert_eq!(
            storage.validate(true).unwrap_err(),
            InvalidHostConfigurationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::DuplicateDeviceId("disk1".into())
            )
        );
    }

    /// Encrypted volumes and partitions must not share the same id
    #[test]
    fn test_validate_encryption_partitions_share_id_fail() {
        let mut storage: Storage = get_storage();
        storage.encryption.as_mut().unwrap().volumes[0].id = "esp".to_owned();
        assert_eq!(
            storage.validate(true).unwrap_err(),
            InvalidHostConfigurationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::DuplicateDeviceId("esp".into())
            )
        );
    }

    /// Encrypted volumes and software RAID arrays must not share the same id
    #[test]
    fn test_validate_encryption_raid_arrays_share_id_fail() {
        let mut storage: Storage = get_storage();
        storage.encryption.as_mut().unwrap().volumes[0].id = "mnt".to_owned();
        assert_eq!(
            storage.validate(true).unwrap_err(),
            InvalidHostConfigurationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::DuplicateDeviceId("mnt".into())
            )
        );
    }

    /// Encrypted volumes and A/B update volume pairs must not share the same id
    #[test]
    fn test_validate_encryption_ab_update_volume_pairs_share_id_fail() {
        let mut storage: Storage = get_storage();
        storage.encryption.as_mut().unwrap().volumes[0].id = "root".to_owned();
        assert_eq!(
            storage.validate(true).unwrap_err(),
            InvalidHostConfigurationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::DuplicateDeviceId("root".into())
            )
        );
    }

    /// Encrypted volumes themselves must not share the same id
    #[test]
    fn test_validate_encryption_volumes_share_id_fail() {
        let mut storage: Storage = get_storage();
        storage.disks[0].partitions.push(Partition {
            id: "alt-enc".to_owned(),
            partition_type: PartitionType::LinuxGeneric,
            size: PartitionSize::from_str("1G").unwrap(),
        });
        storage
            .encryption
            .as_mut()
            .unwrap()
            .volumes
            .push(EncryptedVolume {
                id: "srv".to_owned(),
                device_name: "luks-alt".to_owned(),
                target_id: "alt-enc".to_owned(),
            });

        assert_eq!(
            storage.validate(true).unwrap_err(),
            InvalidHostConfigurationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::DuplicateDeviceId("srv".into())
            )
        );
    }

    /// Encrypted volume device names must be unique
    #[test]
    fn test_validate_encryption_device_names_duplicate_fail() {
        let mut storage: Storage = get_storage();
        storage.disks[0].partitions.push(Partition {
            id: "alt-enc".to_owned(),
            partition_type: PartitionType::LinuxGeneric,
            size: PartitionSize::from_str("1G").unwrap(),
        });
        storage
            .encryption
            .as_mut()
            .unwrap()
            .volumes
            .push(EncryptedVolume {
                id: "alt".to_owned(),
                device_name: "luks-srv".to_owned(),
                target_id: "alt-enc".to_owned(),
            });
        storage.mount_points.push(MountPoint {
            path: PathBuf::from("/alt"),
            filesystem: "ext4".to_owned(),
            options: Vec::new(),
            target_id: "alt".to_owned(),
        });
        assert_eq!(
            storage.validate(true).unwrap_err(),
            InvalidHostConfigurationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::UniqueFieldConstraintError {
                    node_id: "srv".into(),
                    other_id: "alt".into(),
                    kind: BlkDevKind::EncryptedVolume,
                    field_name: "deviceName".into(),
                    value: "luks-srv".into(),
                }
            ),
        );
    }

    /// Encryption recovery key may have file scheme
    #[test]
    fn test_validate_encryption_recovery_key_file_scheme_pass() {
        let mut storage: Storage = get_storage();
        storage.encryption.as_mut().unwrap().recovery_key_url =
            Some(Url::parse("file:///path/to/recovery.key").unwrap());
        storage.validate(true).unwrap();
    }

    /// Encryption recovery key must not have https scheme
    #[test]
    fn test_validate_encryption_recovery_key_http_scheme_fail() {
        let mut storage: Storage = get_storage();
        storage.encryption.as_mut().unwrap().recovery_key_url =
            Some(Url::parse("https://www.example.com/recovery.key").unwrap());
        assert_eq!(
            storage.validate(true).unwrap_err(),
            InvalidHostConfigurationError::InvalidEncryptionRecoveryKeyUrlScheme {
                url: "https://www.example.com/recovery.key".into(),
                scheme: "https".into(),
            }
        );
    }

    /// Encrypted volume target ID may be a home partition
    #[test]
    fn test_validate_encryption_target_id_home_pass() {
        let mut storage: Storage = get_storage();
        storage.disks[1].partitions[5].partition_type = PartitionType::Home;
        storage.validate(true).unwrap();
    }

    /// Encrypted volume target ID must not be an esp partition
    #[test]
    fn test_validate_encryption_target_id_esp_fail() {
        let mut storage: Storage = get_storage();
        storage.encryption.as_mut().unwrap().volumes[0].target_id = "esp".to_owned();
        storage.mount_points.remove(1);
        storage.images.remove(0);
        assert_eq!(
            storage.validate(true).unwrap_err(),
            InvalidHostConfigurationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::InvalidTargets {
                    node_id: "srv".into(),
                    kind: BlkDevKind::EncryptedVolume,
                    body: "Partition 'esp' is of unsupported type 'Esp'.".into()
                }
            ),
        );
    }

    /// Encrypted volume target ID must not be a root partition
    #[test]
    fn test_validate_encryption_target_id_root_fail() {
        let mut storage: Storage = get_storage();
        storage.encryption.as_mut().unwrap().volumes[0].target_id = "root-a".to_owned();
        storage.ab_update.as_mut().unwrap().volume_pairs[0].volume_a_id =
            "root-b-verity".to_owned();
        assert_eq!(
            storage.validate(true).unwrap_err(),
            InvalidHostConfigurationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::InvalidTargets {
                    node_id: "srv".into(),
                    kind: BlkDevKind::EncryptedVolume,
                    body: "Partition 'root-a' is of unsupported type 'Root'.".into()
                }
            ),
        );
    }

    /// Encrypted volume target ID must not be a root-verity partition
    #[test]
    fn test_validate_encryption_target_id_root_verity_fail() {
        let mut storage: Storage = get_storage();
        storage
            .encryption
            .as_mut()
            .unwrap()
            .volumes
            .get_mut(0)
            .unwrap()
            .target_id = "root-b-verity".to_owned();
        assert_eq!(
            storage.validate(true).unwrap_err(),
            InvalidHostConfigurationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::InvalidTargets {
                    node_id: "srv".into(),
                    kind: BlkDevKind::EncryptedVolume,
                    body: "Partition 'root-b-verity' is of unsupported type 'RootVerity'.".into()
                }
            ),
            "Block device 'srv' of kind 'encrypted volume' references invalid targets"
        );
    }

    /// Encrypted volume target ID may be a software RAID array of home partitions
    #[test]
    fn test_validate_encryption_target_id_raid_home_pass() {
        let mut storage: Storage = get_storage();
        storage.encryption.as_mut().unwrap().volumes[0].target_id = "mnt".to_owned();
        storage.mount_points.remove(2);
        storage.disks[1].partitions[3].partition_type = PartitionType::Home;
        storage.disks[1].partitions[4].partition_type = PartitionType::Home;
        storage.validate(true).unwrap();
    }

    /// Encrypted volume target ID must not be a software RAID array of esp partitions
    #[test]
    fn test_validate_encryption_target_id_raid_esp_fail() {
        let mut storage: Storage = get_storage();
        storage.encryption.as_mut().unwrap().volumes[0].target_id = "mnt".to_owned();
        storage.mount_points.remove(2);
        storage.disks[1].partitions[3].partition_type = PartitionType::Esp;
        storage.disks[1].partitions[4].partition_type = PartitionType::Esp;
        storage.validate(true).unwrap();
    }

    /// Encrypted volume target ID must not be a software RAID array of root partitions
    #[test]
    fn test_validate_encryption_target_id_raid_root_fail() {
        let mut storage: Storage = get_storage();
        storage.encryption.as_mut().unwrap().volumes[0].target_id = "mnt".to_owned();
        storage.mount_points.remove(2);
        storage.disks[1].partitions[3].partition_type = PartitionType::Root;
        storage.disks[1].partitions[4].partition_type = PartitionType::Root;
        storage.validate(true).unwrap();
    }

    /// Encrypted volume target ID must not be a software RAID array of root-verity partitions
    #[test]
    fn test_validate_encryption_target_id_raid_root_verity_fail() {
        let mut storage: Storage = get_storage();
        storage.encryption.as_mut().unwrap().volumes[0].target_id = "mnt".to_owned();
        storage.mount_points.remove(2);
        storage
            .disks
            .get_mut(1)
            .unwrap()
            .partitions
            .get_mut(3)
            .unwrap()
            .partition_type = PartitionType::RootVerity;
        storage
            .disks
            .get_mut(1)
            .unwrap()
            .partitions
            .get_mut(4)
            .unwrap()
            .partition_type = PartitionType::RootVerity;
        storage.validate(true).unwrap();
    }

    /// Encrypted volume target ID must not be a software RAID array of no devices.
    #[test]
    fn test_validate_encryption_target_id_raid_no_devices_fail() {
        let mut storage: Storage = get_storage();
        storage.encryption.as_mut().unwrap().volumes[0].target_id = "mnt".to_owned();
        storage.raid.software[0].devices = Vec::new();
        assert_eq!(
            storage.validate(true).unwrap_err(),
            InvalidHostConfigurationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::InvalidTargetCount {
                    node_id: "mnt".into(),
                    kind: BlkDevKind::RaidArray,
                    target_count: 0,
                    expected: BlkDevReferrerKind::RaidArray.valid_target_count()
                }
            ),
        );
    }

    /// Encrypted volume target ID must not be a software RAID array of A/B update volume pairs.
    #[test]
    fn test_validate_encryption_target_id_raid_ab_update_volume_pair_fail() {
        let mut storage: Storage = get_storage();
        storage.encryption.as_mut().unwrap().volumes[0].target_id = "mnt".to_owned();
        storage.raid.software[0].devices = vec!["root".to_owned()];
        // Remove the first mount point
        assert_eq!(
            storage.validate(true).unwrap_err(),
            InvalidHostConfigurationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::InvalidTargetCount {
                    node_id: "mnt".into(),
                    kind: BlkDevKind::RaidArray,
                    target_count: 1,
                    expected: BlkDevReferrerKind::RaidArray.valid_target_count()
                }
            ),
        );
    }

    /// Encrypted volume target ID must not be a disk
    #[test]
    fn test_validate_encryption_target_id_disk_fail() {
        let mut storage: Storage = get_storage();
        storage.encryption.as_mut().unwrap().volumes[0].target_id = "disk1".to_owned();
        assert_eq!(
            storage.validate(true).unwrap_err(),
            InvalidHostConfigurationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::InvalidReferenceKind {
                    node_id: "srv".into(),
                    kind: BlkDevKind::EncryptedVolume,
                    target_id: "disk1".into(),
                    target_kind: BlkDevKind::Disk,
                    valid_references: BlkDevReferrerKind::EncryptedVolume.valid_target_kinds()
                }
            )
        );
    }

    /// Encrypted volume target ID can be a software RAID array instead of a partition
    #[test]
    fn test_validate_encryption_target_id_raid_pass() {
        let mut storage: Storage = get_storage();
        storage.encryption.as_mut().unwrap().volumes[0].target_id = "mnt".to_owned();
        storage.mount_points.remove(2);
        storage.validate(true).unwrap();
    }

    /// Encrypted volume target ID must not be an A/B update volume pair
    #[test]
    fn test_validate_encryption_target_id_ab_update_volume_pair_fail() {
        let mut storage: Storage = get_storage();
        storage.encryption.as_mut().unwrap().volumes[0].target_id = "root".to_owned();
        storage.images.remove(1);
        storage.mount_points.remove(0);
        assert_eq!(
            storage.validate(true).unwrap_err(),
            InvalidHostConfigurationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::InvalidReferenceKind {
                    node_id: "srv".into(),
                    kind: BlkDevKind::EncryptedVolume,
                    target_id: "root".into(),
                    target_kind: BlkDevKind::ABVolume,
                    valid_references: BlkDevReferrerKind::EncryptedVolume.valid_target_kinds()
                }
            ),
        );
    }

    /// Encrypted volume target IDs must be unique
    #[test]
    fn test_validate_encryption_target_id_duplicate_fail() {
        let mut storage: Storage = get_storage();
        storage
            .encryption
            .as_mut()
            .unwrap()
            .volumes
            .push(EncryptedVolume {
                id: "alt".to_owned(),
                device_name: "luks-alt".to_owned(),
                target_id: "srv-enc".to_owned(),
            });
        storage.mount_points.push(MountPoint {
            path: PathBuf::from("/alt"),
            filesystem: "ext4".to_owned(),
            options: Vec::new(),
            target_id: "alt".to_owned(),
        });
        assert_eq!(
            storage.validate(true).unwrap_err(),
            InvalidHostConfigurationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::ReferrerForbiddenSharing {
                    target_id: "srv-enc".into(),
                    target_kind: BlkDevKind::Partition,
                    referrer_a_id: "alt".into(),
                    referrer_a_kind: BlkDevReferrerKind::EncryptedVolume,
                    referrer_a_valid_sharing_peers: BlkDevReferrerKind::EncryptedVolume
                        .valid_sharing_peers(),
                    referrer_b_id: "srv".into(),
                    referrer_b_kind: BlkDevReferrerKind::EncryptedVolume,
                    referrer_b_valid_sharing_peers: BlkDevReferrerKind::EncryptedVolume
                        .valid_sharing_peers(),
                }
            )
        );
    }

    /// Encrypted volumes cannot target the same partition as a mount point
    #[test]
    fn test_validate_encryption_mount_point_target_part_id_equal_fail() {
        let mut storage: Storage = get_storage();
        storage.encryption.as_mut().unwrap().volumes[0].target_id = "mnt-raid-1".to_owned();
        storage.mount_points[2].target_id = "mnt-raid-1".to_owned();
        assert_eq!(
            storage.validate(true).unwrap_err(),
            InvalidHostConfigurationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::ReferrerForbiddenSharing {
                    target_id: "mnt-raid-1".into(),
                    target_kind: BlkDevKind::Partition,
                    referrer_a_id: "mnt".into(),
                    referrer_a_kind: BlkDevReferrerKind::RaidArray,
                    referrer_a_valid_sharing_peers: BlkDevReferrerKind::RaidArray
                        .valid_sharing_peers(),
                    referrer_b_id: "srv".into(),
                    referrer_b_kind: BlkDevReferrerKind::EncryptedVolume,
                    referrer_b_valid_sharing_peers: BlkDevReferrerKind::EncryptedVolume
                        .valid_sharing_peers(),
                }
            ),
        );
    }

    /// Encrypted volumes cannot target the same partition as a software RAID array
    #[test]
    fn test_validate_encryption_software_raid_target_part_id_equal_fail() {
        let mut storage: Storage = get_storage();
        storage.encryption.as_mut().unwrap().volumes[0].target_id = "mnt-raid-1".to_owned();
        assert_eq!(
            storage.validate(true).unwrap_err(),
            InvalidHostConfigurationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::ReferrerForbiddenSharing {
                    target_id: "mnt-raid-1".into(),
                    target_kind: BlkDevKind::Partition,
                    referrer_a_id: "mnt".into(),
                    referrer_a_kind: BlkDevReferrerKind::RaidArray,
                    referrer_a_valid_sharing_peers: BlkDevReferrerKind::RaidArray
                        .valid_sharing_peers(),
                    referrer_b_id: "srv".into(),
                    referrer_b_kind: BlkDevReferrerKind::EncryptedVolume,
                    referrer_b_valid_sharing_peers: BlkDevReferrerKind::EncryptedVolume
                        .valid_sharing_peers(),
                }
            ),
        );
    }

    /// Encrypted volumes cannot target the same partition as A/B update volume pair (A)
    #[test]
    fn test_validate_encryption_ab_update_volume_pair_a_part_id_equal_fail() {
        let mut storage: Storage = get_storage();
        storage.encryption.as_mut().unwrap().volumes[0].target_id = "root-a".to_owned();
        storage.disks[1].partitions[1].partition_type = PartitionType::LinuxGeneric;
        storage.disks[1].partitions[3].partition_type = PartitionType::LinuxGeneric;
        assert_eq!(
            storage.validate(true).unwrap_err(),
            InvalidHostConfigurationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::ReferrerForbiddenSharing {
                    target_id: "root-a".into(),
                    target_kind: BlkDevKind::Partition,
                    referrer_a_id: "root".into(),
                    referrer_a_kind: BlkDevReferrerKind::ABVolume,
                    referrer_a_valid_sharing_peers: BlkDevReferrerKind::ABVolume
                        .valid_sharing_peers(),
                    referrer_b_id: "srv".into(),
                    referrer_b_kind: BlkDevReferrerKind::EncryptedVolume,
                    referrer_b_valid_sharing_peers: BlkDevReferrerKind::EncryptedVolume
                        .valid_sharing_peers(),
                }
            ),
        );
    }

    /// Encrypted volumes cannot target the same partition as A/B update volume pair (B)
    #[test]
    fn test_validate_encryption_ab_update_volume_pair_b_part_id_equal_fail() {
        let mut storage: Storage = get_storage();
        storage.encryption.as_mut().unwrap().volumes[0].target_id = "root-b".to_owned();
        storage.disks[1].partitions[1].partition_type = PartitionType::LinuxGeneric;
        storage.disks[1].partitions[3].partition_type = PartitionType::LinuxGeneric;
        assert_eq!(
            storage.validate(true).unwrap_err(),
            InvalidHostConfigurationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::ReferrerForbiddenSharing {
                    target_id: "root-b".into(),
                    target_kind: BlkDevKind::Partition,
                    referrer_a_id: "root".into(),
                    referrer_a_kind: BlkDevReferrerKind::ABVolume,
                    referrer_a_valid_sharing_peers: BlkDevReferrerKind::ABVolume
                        .valid_sharing_peers(),
                    referrer_b_id: "srv".into(),
                    referrer_b_kind: BlkDevReferrerKind::EncryptedVolume,
                    referrer_b_valid_sharing_peers: BlkDevReferrerKind::EncryptedVolume
                        .valid_sharing_peers(),
                }
            ),
        );
    }

    /// Encrypted volumes cannot target the same software RAID array as an A/B update volume pair (A)
    #[test]
    fn test_validate_encryption_ab_update_volume_pair_a_raid_id_equal_fail() {
        let mut storage: Storage = get_storage();
        storage.encryption.as_mut().unwrap().volumes[0].target_id = "mnt".to_owned();
        storage.mount_points.remove(2); // remove /mnt mount point
        storage.ab_update.as_mut().unwrap().volume_pairs[0].volume_a_id = "mnt".to_owned();
        assert_eq!(
            storage.validate(true).unwrap_err(),
            InvalidHostConfigurationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::ReferrerForbiddenSharing {
                    target_id: "mnt".into(),
                    target_kind: BlkDevKind::RaidArray,
                    referrer_a_id: "root".into(),
                    referrer_a_kind: BlkDevReferrerKind::ABVolume,
                    referrer_a_valid_sharing_peers: BlkDevReferrerKind::ABVolume
                        .valid_sharing_peers(),
                    referrer_b_id: "srv".into(),
                    referrer_b_kind: BlkDevReferrerKind::EncryptedVolume,
                    referrer_b_valid_sharing_peers: BlkDevReferrerKind::EncryptedVolume
                        .valid_sharing_peers(),
                }
            ),
        );
    }

    /// Encrypted volumes cannot target the same software RAID array as an A/B update volume pair (B)
    #[test]
    fn test_validate_encryption_ab_update_volume_pair_b_raid_id_equal_fail() {
        let mut storage: Storage = get_storage();
        storage.encryption.as_mut().unwrap().volumes[0].target_id = "mnt".to_owned();
        storage.mount_points.remove(2); // remove /mnt mount point
        storage.ab_update.as_mut().unwrap().volume_pairs[0].volume_b_id = "mnt".to_owned();
        assert_eq!(
            storage.validate(true).unwrap_err(),
            InvalidHostConfigurationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::ReferrerForbiddenSharing {
                    target_id: "mnt".into(),
                    target_kind: BlkDevKind::RaidArray,
                    referrer_a_id: "root".into(),
                    referrer_a_kind: BlkDevReferrerKind::ABVolume,
                    referrer_a_valid_sharing_peers: BlkDevReferrerKind::ABVolume
                        .valid_sharing_peers(),
                    referrer_b_id: "srv".into(),
                    referrer_b_kind: BlkDevReferrerKind::EncryptedVolume,
                    referrer_b_valid_sharing_peers: BlkDevReferrerKind::EncryptedVolume
                        .valid_sharing_peers(),
                }
            ),
        );
    }

    /// Encrypted volumes cannot target the same partition as an image
    #[test]
    fn test_validate_encryption_image_target_part_id_equal_fail() {
        let mut storage: Storage = get_storage();
        storage.images[0].target_id = "srv-enc".to_owned();
        assert_eq!(
            storage.validate(true).unwrap_err(),
            InvalidHostConfigurationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::ReferrerForbiddenSharing {
                    target_id: "srv-enc".into(),
                    target_kind: BlkDevKind::Partition,
                    referrer_a_id: "file:///esp.raw.zst".into(),
                    referrer_a_kind: BlkDevReferrerKind::Image,
                    referrer_a_valid_sharing_peers: BlkDevReferrerKind::Image.valid_sharing_peers(),
                    referrer_b_id: "srv".into(),
                    referrer_b_kind: BlkDevReferrerKind::EncryptedVolume,
                    referrer_b_valid_sharing_peers: BlkDevReferrerKind::EncryptedVolume
                        .valid_sharing_peers(),
                }
            ),
        );
    }

    /// Image must be an A/B update volume pair if format is raw-lzma
    #[test]
    #[cfg(feature = "sysupdate")]
    fn test_validate_image_raw_lzma_ab_update_volume_pair_pass() {
        let mut storage: Storage = get_storage();
        storage.images[1].format = ImageFormat::RawLzma;
        storage.validate(true).unwrap();
    }

    /// Image must not be a partition if format is raw-lzma
    #[test]
    #[cfg(feature = "sysupdate")]
    fn test_validate_image_raw_lzma_partition_fail() {
        let mut storage: Storage = get_storage();
        storage.images[0].format = ImageFormat::RawLzma;
        assert_eq!(
            storage.validate(true).unwrap_err(),
            InvalidHostConfigurationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::ImageInvalidReference {
                    image_id: "file:///esp.raw.zst".into(),
                    target_id: "esp".into(),
                    target_kind: BlkDevKind::Partition,
                    valid_references: BlkDevReferrerKind::ImageSysupdate.valid_target_kinds()
                }
            )
        );
    }

    /// Images can target encrypted volumes
    #[test]
    fn test_validate_image_target_id_encryption_pass() {
        let mut storage: Storage = get_storage();
        let mut images = storage.images.clone();
        images.push(Image {
            target_id: "srv".to_owned(),
            url: "file:///root.raw.zst".to_owned(),
            sha256: ImageSha256::Ignored,
            format: ImageFormat::RawZst,
        });
        storage.images = images;
        storage.validate(true).unwrap();
    }

    #[test]
    fn test_validate_verity_pass() {
        let mut storage: Storage = get_storage();
        storage.verity = vec![VerityDevice {
            id: "verity-root-a".to_owned(),
            device_name: "root".to_owned(),
            data_target_id: "root-a".to_owned(),
            hash_target_id: "root-a-verity".to_owned(),
        }];
        storage.mount_points[0].target_id = "verity-root-a".into();
        storage.mount_points[0].options.push("ro".to_owned());
        storage.images[1].target_id = "root-a".into();
        storage.ab_update = None;
        storage.validate(true).unwrap();
    }

    #[test]
    fn test_validate_verity_rw_fail() {
        let mut storage: Storage = get_storage();
        storage.verity = vec![VerityDevice {
            id: "verity-root-a".to_owned(),
            device_name: "root".to_owned(),
            data_target_id: "root-a".to_owned(),
            hash_target_id: "root-a-verity".to_owned(),
        }];
        storage.mount_points[0].target_id = "verity-root-a".into();
        storage.images[1].target_id = "root-a".into();
        storage.ab_update = None;
        assert_eq!(
            storage.validate(true).unwrap_err(),
            InvalidHostConfigurationError::VerityDeviceReadWrite {
                mount_point_path: "/".into(),
                device_name: "root".into(),
            }
        );
    }

    #[test]
    fn test_validate_verity_bad_device_name_fail() {
        let mut storage: Storage = get_storage();
        storage.verity = vec![VerityDevice {
            id: "verity-root-a".to_owned(),
            device_name: "verity-root-a".to_owned(),
            data_target_id: "root-a".to_owned(),
            hash_target_id: "root-a-verity".to_owned(),
        }];
        storage.mount_points[0].target_id = "verity-root-a".into();
        storage.images[1].target_id = "root-a".into();
        storage.ab_update = None;
        assert_eq!(
            storage.validate(true).unwrap_err(),
            InvalidHostConfigurationError::RootVerityDeviceNameInvalid {
                device_name: "verity-root-a".into()
            }
        );
    }

    #[test]
    fn test_validate_verity_without_boot_image_fail() {
        let mut storage: Storage = get_storage();
        storage.verity = vec![VerityDevice {
            id: "verity-root-a".to_owned(),
            device_name: "verity-root-a".to_owned(),
            data_target_id: "root-a".to_owned(),
            hash_target_id: "root-a-verity".to_owned(),
        }];
        storage.mount_points[0].target_id = "verity-root-a".into();
        storage.images[1].target_id = "root-a".into();
        storage.ab_update = None;

        storage.images.remove(3);
        assert_eq!(
            storage.validate(true).unwrap_err(),
            InvalidHostConfigurationError::MountPointNotBackedByImage {
                mount_point_path: "/boot".into()
            },
        );
    }

    #[test]
    fn test_validate_verity_without_boot_mountpoint_fail() {
        let mut storage: Storage = get_storage();
        storage.verity = vec![VerityDevice {
            id: "verity-root-a".to_owned(),
            device_name: "verity-root-a".to_owned(),
            data_target_id: "root-a".to_owned(),
            hash_target_id: "root-a-verity".to_owned(),
        }];
        storage.mount_points[0].target_id = "verity-root-a".into();
        storage.images[1].target_id = "root-a".into();
        storage.ab_update = None;

        storage.images.remove(3);
        storage.mount_points.remove(4);
        assert_eq!(
            storage.validate(true).unwrap_err(),
            InvalidHostConfigurationError::ExpectedMountPointNotFound {
                mount_point_path: "/boot".into()
            },
        );
    }

    #[test]
    fn test_validate_verity_without_hash_image_fail() {
        let mut storage: Storage = get_storage();
        storage.verity = vec![VerityDevice {
            id: "verity-root-a".to_owned(),
            device_name: "root".to_owned(),
            data_target_id: "root-a".to_owned(),
            hash_target_id: "root-a-verity".to_owned(),
        }];
        storage.mount_points[0].target_id = "verity-root-a".into();
        storage.images[1].target_id = "root-a".into();
        storage.ab_update = None;

        storage.images.remove(2);
        assert_eq!(
            storage.validate(true).unwrap_err(),
            InvalidHostConfigurationError::InvalidBlockDeviceGraph(BlockDeviceGraphBuildError::InvalidTargets { node_id: "verity-root-a".into(), kind: BlkDevKind::VerityDevice, body: "Verity device 'verity-root-a' points to a block device that has not been initialized with an image: Block device 'root-a-verity' is not initialized using image, which is required for verity device 'verity-root-a' to work".into() }),
        );
    }

    #[test]
    fn test_validate_verity_ro_overlay_fail() {
        let mut storage: Storage = get_storage();
        storage.verity = vec![VerityDevice {
            id: "verity-root-a".to_owned(),
            device_name: "root".to_owned(),
            data_target_id: "root-a".to_owned(),
            hash_target_id: "root-a-verity".to_owned(),
        }];
        storage.mount_points[0].target_id = "verity-root-a".into();
        storage.images[1].target_id = "root-a".into();
        storage.ab_update = None;
        storage.mount_points.remove(5);
        assert_eq!(
            storage.validate(true).unwrap_err(),
            InvalidHostConfigurationError::OverlayOnReadOnlyVolume {
                mount_point_path: "/".into(),
                overlay_path: "/var/lib/trident-overlay".into()
            }
        );
    }

    #[test]
    fn test_validate_verity_ro_overlay_2_fail() {
        let mut storage: Storage = get_storage();
        storage.verity = vec![VerityDevice {
            id: "verity-root-a".to_owned(),
            device_name: "root".to_owned(),
            data_target_id: "root-a".to_owned(),
            hash_target_id: "root-a-verity".to_owned(),
        }];
        storage.mount_points[0].target_id = "verity-root-a".into();
        storage.images[1].target_id = "root-a".into();
        storage.ab_update = None;
        storage.mount_points[0].options.push("ro".to_owned());
        storage.mount_points[5].options.push("ro".to_owned());
        assert_eq!(
            storage.validate(true).unwrap_err(),
            InvalidHostConfigurationError::OverlayOnReadOnlyVolume {
                mount_point_path: "/var/lib/trident-overlay".into(),
                overlay_path: "/var/lib/trident-overlay".into()
            }
        );
    }

    #[test]
    fn test_path_to_mount_point() {
        let mut host_config = HostConfiguration {
            storage: Storage {
                disks: vec![
                    Disk {
                        id: "disk1".to_owned(),
                        device: ROOT_MOUNT_POINT_PATH.into(),
                        ..Default::default()
                    },
                    Disk {
                        id: "disk2".to_owned(),
                        device: "/tmp".into(),
                        partitions: vec![
                            Partition {
                                id: "part1".to_owned(),
                                partition_type: PartitionType::Esp,
                                size: PartitionSize::from_str("1M").unwrap(),
                            },
                            Partition {
                                id: "part2".to_owned(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::from_str("1G").unwrap(),
                            },
                            Partition {
                                id: "part3".to_owned(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::from_str("1G").unwrap(),
                            },
                            Partition {
                                id: "part4".to_owned(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::from_str("1G").unwrap(),
                            },
                            Partition {
                                id: "part5".to_owned(),
                                partition_type: PartitionType::Srv,
                                size: PartitionSize::from_str("1G").unwrap(),
                            },
                        ],
                        ..Default::default()
                    },
                ],
                raid: Raid {
                    software: vec![SoftwareRaidArray {
                        id: "my-raid1".to_owned(),
                        name: "my-raid".to_owned(),
                        level: RaidLevel::Raid1,
                        metadata_version: "1.2".to_owned(),
                        devices: vec!["part3".to_owned(), "part4".to_owned()],
                    }],
                },
                mount_points: vec![MountPoint {
                    filesystem: "ext4".to_owned(),
                    options: vec![],
                    target_id: "part1".to_owned(),
                    path: PathBuf::from(ROOT_MOUNT_POINT_PATH),
                }],
                images: vec![Image {
                    target_id: "part1".to_owned(),
                    url: "".to_owned(),
                    sha256: ImageSha256::Ignored,
                    format: ImageFormat::RawZst,
                }],
                ab_update: Some(AbUpdate {
                    volume_pairs: vec![AbVolumePair {
                        id: "ab1".to_owned(),
                        volume_a_id: "part1".to_owned(),
                        volume_b_id: "part2".to_owned(),
                    }],
                }),
                encryption: None,
                verity: vec![],
            },
            ..Default::default()
        };
        let mount_point = host_config
            .storage
            .path_to_mount_point(Path::new(ROOT_MOUNT_POINT_PATH).join("boot").as_path())
            .unwrap();
        assert_eq!(mount_point.target_id, "part1");

        // ensure to pick the longest prefix
        host_config.storage.mount_points.push(MountPoint {
            filesystem: "ext4".to_owned(),
            options: vec![],
            target_id: "part2".to_owned(),
            path: PathBuf::from(ROOT_MOUNT_POINT_PATH)
                .join("boot")
                .as_path()
                .into(),
        });

        let mount_point = host_config
            .storage
            .path_to_mount_point(Path::new(ROOT_MOUNT_POINT_PATH).join("boot").as_path())
            .unwrap();
        assert_eq!(mount_point.target_id, "part2");

        // validate longer paths
        let mount_point = host_config
            .storage
            .path_to_mount_point(
                Path::new(ROOT_MOUNT_POINT_PATH)
                    .join("boot/foo/bar")
                    .as_path(),
            )
            .unwrap();
        assert_eq!(mount_point.target_id, "part2");

        let mount_point = host_config
            .storage
            .path_to_mount_point(Path::new(ROOT_MOUNT_POINT_PATH).join("foo/bar").as_path())
            .unwrap();
        assert_eq!(mount_point.target_id, "part1");

        // validate failure without any mount points
        host_config.storage.mount_points.clear();
        assert!(host_config
            .storage
            .path_to_mount_point(Path::new(ROOT_MOUNT_POINT_PATH).join("boot").as_path())
            .is_none());
    }
}
