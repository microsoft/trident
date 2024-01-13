use std::path::PathBuf;

use anyhow::{ensure, Context, Error};
use serde::{Deserialize, Serialize};
use strum_macros::{Display, EnumString};

#[cfg(feature = "schemars")]
use schemars::JsonSchema;
use url::Url;

use crate::{is_default, BlockDeviceId};

#[cfg(feature = "schemars")]
use crate::schema_helpers::{block_device_id_list_schema, block_device_id_schema};

pub mod blkdev_graph;
pub mod imaging;
pub mod partitions;
mod serde_hash;

use partitions::{AdoptedPartition, Partition};

use imaging::{AbUpdate, Image};

use self::blkdev_graph::{
    builder::BlockDeviceGraphBuilder, error::BlockDeviceGraphBuildError, graph::BlockDeviceGraph,
};

/// Storage configuration describes the disks of the host that will be used to
/// store the OS and data. Not all disks of the host need to be captured inside
/// the Host Configuration, only those that Trident should operate on.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq)]
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
    pub raid: RaidConfig,

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

/// Per disk configuration.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct Disk {
    /// A unique identifier for the disk. This is a user defined string that
    /// allows to link the disk to what is consuming it and also to results in the
    /// Host Status. The identifier needs to be unique across all types of
    /// devices, not just disks.
    ///
    /// TBD: At the moment, the partition table is created from scratch. In the
    /// future, it will be possible to consume an existing partition table.
    #[cfg_attr(feature = "schemars", schemars(schema_with = "block_device_id_schema"))]
    pub id: BlockDeviceId,

    /// The device path of the disk. Points to the disk device in the host. It is
    /// recommended to use stable paths, such as the ones under `/dev/disk/by-path/`
    /// or [WWNs](https://en.wikipedia.org/wiki/World_Wide_Name).
    pub device: PathBuf,

    /// The partition table type of the disk. Supported values are: `gpt`.
    pub partition_table_type: PartitionTableType,

    /// A list of partitions that will be created on the disk.
    pub partitions: Vec<Partition>,

    /// A list of pre-existing partitions that will be adopted from the disk.
    ///
    /// Several options are available to match a partition to adopt. If more
    /// than one option is specified, ALL the provided criteria will be used to
    /// match the partition.
    #[serde(default)]
    pub adopted_partitions: Vec<AdoptedPartition>,
}

/// Partition table type. Currently only GPT is supported.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub enum PartitionTableType {
    /// # GPT
    ///
    /// Disk should be formatted with a GUID Partition Table (GPT).
    #[default]
    Gpt,
}

/// Configure encrypted volumes of underlying disk partitions or software
/// raid arrays.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct Encryption {
    /// A URL to the file containing the recovery key to use for
    /// encryption.
    ///
    /// This parameter is optional but highly encouraged. If not
    /// specified, only the TPM2 device will be enrolled.
    ///
    /// `file` is the only currently supported URL scheme. The contents of
    /// the key serve as the key. It must be in plain text and not
    /// encoded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery_key_url: Option<Url>,

    /// The list of LUKS2-encrypted volumes to create.
    ///
    /// This parameter is required and must not be empty. Each item is an
    /// object that will contain the configuration for a given partition
    /// or RAID array.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub volumes: Vec<EncryptedVolume>,
}

/// A LUKS2-encrypted volume configuration.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct EncryptedVolume {
    /// The id of the LUKS-encrypted volumes to create.
    ///
    /// This parameter is required. It must be non-empty and unique among
    /// the ids of all block devices in the host configuration. This
    /// includes the ids of all disk partitions, encrypted volumes,
    /// software raid arrays, and a/b upgrade volume pairs.
    #[cfg_attr(feature = "schemars", schemars(schema_with = "block_device_id_schema"))]
    pub id: BlockDeviceId,

    /// The name of the device to create under `/dev/mapper` when opening
    /// the volume.
    ///
    /// This parameter is required. It must be a valid file name and
    /// unique among the list of encrypted volumes.
    pub device_name: String,

    /// The id of the disk partition or software raid array to encrypt.
    ///
    /// This parameter is required. It must be unique among the list of
    /// encrypted volumes.
    ///
    /// If it refers to a disk partition, it must be of a supported type.
    /// Supported types are all but `root` and `efi`.
    ///
    /// If it refers to a software raid array, the first disk partition of
    /// the software raid array must be of a supported type.
    #[cfg_attr(feature = "schemars", schemars(schema_with = "block_device_id_schema"))]
    pub target_id: BlockDeviceId,
}

/// RAID configuration for a host.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct RaidConfig {
    /// Individual software raid configurations.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub software: Vec<SoftwareRaidArray>,
}

/// Software RAID configuration.
///
/// The RAID array will be created using the `mdadm` package. During a clean
/// install, all the existing RAID arrays that are on disks defined in the host
/// configuration will be unmounted, and stopped.
///
/// The RAID arrays that are defined in the host configuration will be created,
/// and mounted if specified in `mount-points`.
///
/// To learn more about RAID, please refer to the [RAID
/// wiki](https://wiki.archlinux.org/title/RAID)
///
/// To learn more about `mdadm`, please refer to the [mdadm
/// guide](https://raid.wiki.kernel.org/index.php/A_guide_to_mdadm)
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct SoftwareRaidArray {
    /// A unique identifier for the RAID array.
    ///
    /// This is a user defined string that allows to link the RAID array to the
    /// mount points and also to results in the Host Status. The identifier
    /// needs to be unique across all types of devices, not just RAID arrays.
    #[cfg_attr(feature = "schemars", schemars(schema_with = "block_device_id_schema"))]
    pub id: BlockDeviceId,

    /// Name of the RAID array.
    ///
    /// This is used to reference the RAID array on the system. For example,
    /// `some-raid` will result in `/dev/md/some-raid` on the system.
    pub name: String,

    /// RAID level.
    ///
    /// Supported and tested values are `raid0`, `raid1`.
    /// Other possible values yet to be tested are: `raid5`, `raid6`, `raid10`.
    pub level: RaidLevel,

    /// Devices that will be used for the RAID array.
    ///
    /// See the reference links for picking the right number of devices. Devices
    /// are partition ids from the `disks` section.
    #[cfg_attr(
        feature = "schemars",
        schemars(schema_with = "block_device_id_list_schema")
    )]
    pub devices: Vec<BlockDeviceId>,

    /// Metadata of the RAID array.
    ///
    /// Supported and tested values are `1.0`. Note that this is a string attribute.
    pub metadata_version: String,
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, Hash, Eq, PartialEq, Display, EnumString)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub enum RaidLevel {
    /// # Striping
    #[strum(serialize = "0")]
    Raid0,

    /// # Mirroring
    #[strum(serialize = "1")]
    Raid1,

    /// # Striping with parity
    #[strum(serialize = "5")]
    Raid5,

    /// # Striping with double parity
    #[strum(serialize = "6")]
    Raid6,

    /// # Stripe of mirrors
    #[strum(serialize = "10")]
    Raid10,
}

/// Mount point configuration.
///
/// These are used by Trident to update the `/etc/fstab` in the runtime OS to
/// correctly mount the volumes.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct MountPoint {
    /// The path of the mount point.
    ///
    /// This is the path where the volume will be mounted in the runtime OS.
    /// For `swap` partitions, the path should be `none`.
    pub path: PathBuf,

    /// The filesystem to be used for this mount point.
    ///
    /// This value will be used to format the partition.
    pub filesystem: String,

    /// A list of options to be used for this mount point.
    ///
    /// These will be passed as is to the `/etc/fstab` file.
    pub options: Vec<String>,

    /// The id of the block device that will be mounted at this mount
    /// point.
    ///
    /// This parameter is required. It must be the ID of a disk partition,
    /// encrypted volume, software raid array, or a/b update volume pair.
    #[cfg_attr(feature = "schemars", schemars(schema_with = "block_device_id_schema"))]
    pub target_id: BlockDeviceId,
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
    pub fn validate(&self) -> Result<(), Error> {
        // Check basic constraints

        // Check encryption settings
        if let Some(encryption) = &self.encryption {
            // Encryption recovery key URLs must start with file://
            if let Some(recovery_key_url) = &encryption.recovery_key_url {
                ensure!(
                    recovery_key_url.scheme() == "file",
                    "Encryption recovery key URL '{}' has an invalid scheme '{}'",
                    recovery_key_url,
                    recovery_key_url.scheme()
                );
            }
        }

        // Build the graph
        self.build_graph()
            .map(|_| ())
            .context("Storage configuration is invalid")
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use imaging::{AbVolumePair, ImageFormat, ImageSha256};
    use partitions::{PartitionSize, PartitionType};

    use super::*;

    /// Generate a basic valid Storage configuration for testing.
    fn test_storage() -> Storage {
        Storage {
            disks: vec![
                Disk {
                    id: "disk1".to_owned(),
                    device: "/".into(),
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
            raid: RaidConfig {
                software: vec![SoftwareRaidArray {
                    id: "mnt".to_owned(),
                    name: "md-mnt".to_owned(),
                    level: RaidLevel::Raid1,
                    metadata_version: "1.2".to_owned(),
                    devices: vec!["mnt-raid-1".to_owned(), "mnt-raid-2".to_owned()],
                }],
            },
            mount_points: vec![
                MountPoint {
                    path: PathBuf::from("/"),
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
            ],
            images: vec![
                Image {
                    target_id: "esp".to_owned(),
                    url: "file:///esp.raw.zst".to_owned(),
                    sha256: ImageSha256::Ignored,
                    format: ImageFormat::RawZstd,
                },
                Image {
                    target_id: "root".to_owned(),
                    url: "file:///root.raw.zst".to_owned(),
                    sha256: ImageSha256::Ignored,
                    format: ImageFormat::RawZstd,
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
            ..Default::default()
        };
        storage.validate().unwrap();

        let mount_volume_pair = Storage {
            ab_update: Some(AbUpdate {
                volume_pairs: vec![imaging::AbVolumePair {
                    id: "ab-update-volume-pair".to_string(),
                    volume_a_id: "disk1-partition2".to_string(),
                    volume_b_id: "disk2-partition2".to_string(),
                }],
            }),
            mount_points: vec![MountPoint {
                filesystem: "ext4".to_string(),
                options: vec![],
                target_id: "ab-update-volume-pair".to_string(),
                path: PathBuf::from("/"),
            }],
            ..storage.clone()
        };
        mount_volume_pair.validate().unwrap();

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
        assert!(bad_volume_pair.validate().is_err());

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
        assert!(bad_volume_pair_id.validate().is_err());

        let bad_image_target = Storage {
            images: vec![Image {
                format: imaging::ImageFormat::RawZstd,
                target_id: "disk99".to_string(),
                url: "http://example.com/image".to_string(),
                sha256: imaging::ImageSha256::Ignored,
            }],
            ..storage.clone()
        };
        assert!(bad_image_target.validate().is_err());
    }

    #[test]
    fn test_validate2() {
        Storage::default().validate().unwrap();

        let mut storage = Storage {
            disks: vec![
                Disk {
                    id: "disk1".to_owned(),
                    device: "/".into(),
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
                    ],
                    ..Default::default()
                },
            ],
            raid: RaidConfig {
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
                target_id: "ab1".to_owned(),
                path: PathBuf::from("/"),
            }],
            images: vec![Image {
                target_id: "ab1".to_owned(),
                url: "https://some/url".to_owned(),
                sha256: imaging::ImageSha256::Checksum("".into()),
                format: ImageFormat::RawZstd,
            }],
            ab_update: Some(AbUpdate {
                volume_pairs: vec![AbVolumePair {
                    id: "ab1".to_owned(),
                    volume_a_id: "part1".to_owned(),
                    volume_b_id: "part2".to_owned(),
                }],
            }),
            encryption: None,
        };
        storage.validate().unwrap();

        let storage_golden = storage.clone();

        // fail on duplicate id
        storage = storage_golden.clone();
        storage.disks.get_mut(0).unwrap().partitions = vec![Partition {
            id: "part1".to_owned(),
            partition_type: PartitionType::Esp,
            size: PartitionSize::from_str("1M").unwrap(),
        }];
        assert!(storage.validate().is_err());

        // fail on duplicate id
        storage = storage_golden.clone();
        storage.ab_update.as_mut().unwrap().volume_pairs[0].id = "disk1".to_owned();
        assert!(storage.validate().is_err());

        // fail on missing reference (disk4 does not exist)
        storage = storage_golden.clone();
        storage.ab_update.as_mut().unwrap().volume_pairs[0].volume_a_id = "disk4".to_owned();
        assert!(storage.validate().is_err());

        // fail on missing reference (disk4 does not exist)
        storage = storage_golden.clone();
        storage.images[0].target_id = "disk4".to_owned();
        assert!(storage.validate().is_err());

        // fail on missing reference (disk4 does not exist)
        storage = storage_golden.clone();
        storage.mount_points[0].target_id = "disk4".to_owned();
        assert!(storage.validate().is_err());

        // fail on bad block device type
        storage = storage_golden.clone();
        storage.images[0].target_id = "disk1".to_owned();
        assert!(storage.validate().is_err());

        // fail if devices are not all the same size for a RAID
        storage.disks[1].partitions[3].size = PartitionSize::from_str("2G").unwrap();
        assert!(storage.validate().is_err());
    }

    #[test]
    fn test_validate_encryption_pass() {
        let storage: Storage = test_storage();
        storage.validate().unwrap();
    }

    /// A/B update volume pairs can target encrypted volumes (A)
    #[test]
    fn test_validate_ab_update_volume_pair_a_id_encryption_pass() {
        let mut storage: Storage = test_storage();
        storage.ab_update.as_mut().unwrap().volume_pairs[0].volume_a_id = "srv".to_owned();
        // Delete mount point associated with "srv", otherwise this would fail
        storage.mount_points.retain(|mp| mp.target_id != "srv");
        storage.validate().unwrap();
    }

    /// A/B update volume pairs can target encrypted volumes (B)
    #[test]
    fn test_validate_ab_update_volume_pair_b_id_encryption_pass() {
        let mut storage: Storage = test_storage();
        storage.ab_update.as_mut().unwrap().volume_pairs[0].volume_b_id = "srv".to_owned();
        // Delete mount point associated with "srv", otherwise this would fail
        storage.mount_points.retain(|mp| mp.target_id != "srv");
        storage.validate().unwrap();
    }

    /// Software RAID arrays must have one or more devices
    #[test]
    fn test_validate_software_raid_array_no_devices_fail() {
        let mut storage: Storage = test_storage();
        storage.raid.software[0].devices = Vec::new();
        storage.validate().unwrap_err();
    }

    /// Software RAID arrays cannot target encrypted volumes
    #[test]
    fn test_validate_software_raid_target_id_encryption_fail() {
        let mut storage: Storage = test_storage();
        storage.raid.software[0].devices[0] = "srv".to_owned();
        eprintln!("{:?}", storage.validate().unwrap_err());
        storage.validate().unwrap_err();
    }

    /// Encrypted volumes and disks must not share the same id
    #[test]
    fn test_validate_encryption_disks_share_id_fail() {
        let mut storage: Storage = test_storage();
        storage.encryption.as_mut().unwrap().volumes[0].id = "disk1".to_owned();
        storage.validate().unwrap_err();
    }

    /// Encrypted volumes and partitions must not share the same id
    #[test]
    fn test_validate_encryption_partitions_share_id_fail() {
        let mut storage: Storage = test_storage();
        storage.encryption.as_mut().unwrap().volumes[0].id = "esp".to_owned();
        storage.validate().unwrap_err();
    }

    /// Encrypted volumes and software RAID arrays must not share the same id
    #[test]
    fn test_validate_encryption_raid_arrays_share_id_fail() {
        let mut storage: Storage = test_storage();
        storage.encryption.as_mut().unwrap().volumes[0].id = "mnt".to_owned();
        storage.validate().unwrap_err();
    }

    /// Encrypted volumes and A/B update volume pairs must not share the same id
    #[test]
    fn test_validate_encryption_ab_update_volume_pairs_share_id_fail() {
        let mut storage: Storage = test_storage();
        storage.encryption.as_mut().unwrap().volumes[0].id = "root".to_owned();
        storage.validate().unwrap_err();
    }

    /// Encrypted volumes themselves must not share the same id
    #[test]
    fn test_validate_encryption_volumes_share_id_fail() {
        let mut storage: Storage = test_storage();
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

        storage.validate().unwrap_err();
    }

    /// Encrypted volume device names must be unique
    #[test]
    fn test_validate_encryption_device_names_duplicate_fail() {
        let mut storage: Storage = test_storage();
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
        storage.validate().unwrap_err();
    }

    /// Encryption recovery key may have file scheme
    #[test]
    fn test_validate_encryption_recovery_key_file_scheme_pass() {
        let mut storage: Storage = test_storage();
        storage.encryption.as_mut().unwrap().recovery_key_url =
            Some(Url::parse("file:///path/to/recovery.key").unwrap());
        storage.validate().unwrap();
    }

    /// Encryption recovery key must not have https scheme
    #[test]
    fn test_validate_encryption_recovery_key_http_scheme_fail() {
        let mut storage: Storage = test_storage();
        storage.encryption.as_mut().unwrap().recovery_key_url =
            Some(Url::parse("https://www.example.com/recovery.key").unwrap());
        storage.validate().unwrap_err();
    }

    /// Encrypted volume target ID may be a home partition
    #[test]
    fn test_validate_encryption_target_id_home_pass() {
        let mut storage: Storage = test_storage();
        storage.disks[1].partitions[5].partition_type = PartitionType::Home;
        storage.validate().unwrap();
    }

    /// Encrypted volume target ID must not be an esp partition
    #[test]
    fn test_validate_encryption_target_id_esp_fail() {
        let mut storage: Storage = test_storage();
        storage.encryption.as_mut().unwrap().volumes[0].target_id = "esp".to_owned();
        storage.mount_points.remove(1);
        storage.images.remove(0);
        storage.validate().unwrap_err();
    }

    /// Encrypted volume target ID must not be a root partition
    #[test]
    fn test_validate_encryption_target_id_root_fail() {
        let mut storage: Storage = test_storage();
        storage.encryption.as_mut().unwrap().volumes[0].target_id = "root-a".to_owned();
        storage.ab_update.as_mut().unwrap().volume_pairs[0].volume_a_id =
            "root-a-verity".to_owned();
        storage.validate().unwrap_err();
    }

    /// Encrypted volume target ID must not be a root-verity partition
    #[test]
    fn test_validate_encryption_target_id_root_verity_fail() {
        let mut storage: Storage = test_storage();
        storage
            .encryption
            .as_mut()
            .unwrap()
            .volumes
            .get_mut(0)
            .unwrap()
            .target_id = "root-a-verity".to_owned();
        storage.validate().unwrap_err();
    }

    /// Encrypted volume target ID may be a software RAID array of home partitions
    #[test]
    fn test_validate_encryption_target_id_raid_home_pass() {
        let mut storage: Storage = test_storage();
        storage.encryption.as_mut().unwrap().volumes[0].target_id = "mnt".to_owned();
        storage.mount_points.remove(2);
        storage.disks[1].partitions[3].partition_type = PartitionType::Home;
        storage.disks[1].partitions[4].partition_type = PartitionType::Home;
        storage.validate().unwrap();
    }

    /// Encrypted volume target ID must not be a software RAID array of esp partitions
    #[test]
    fn test_validate_encryption_target_id_raid_esp_fail() {
        let mut storage: Storage = test_storage();
        storage.encryption.as_mut().unwrap().volumes[0].target_id = "mnt".to_owned();
        storage.mount_points.remove(2);
        storage.disks[1].partitions[3].partition_type = PartitionType::Esp;
        storage.disks[1].partitions[4].partition_type = PartitionType::Esp;
        storage.validate().unwrap();
    }

    /// Encrypted volume target ID must not be a software RAID array of root partitions
    #[test]
    fn test_validate_encryption_target_id_raid_root_fail() {
        let mut storage: Storage = test_storage();
        storage.encryption.as_mut().unwrap().volumes[0].target_id = "mnt".to_owned();
        storage.mount_points.remove(2);
        storage.disks[1].partitions[3].partition_type = PartitionType::Root;
        storage.disks[1].partitions[4].partition_type = PartitionType::Root;
        storage.validate().unwrap();
    }

    /// Encrypted volume target ID must not be a software RAID array of root-verity partitions
    #[test]
    fn test_validate_encryption_target_id_raid_root_verity_fail() {
        let mut storage: Storage = test_storage();
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
        storage.validate().unwrap();
    }

    /// Encrypted volume target ID must not be a software RAID array of no devices.
    #[test]
    fn test_validate_encryption_target_id_raid_no_devices_fail() {
        let mut storage: Storage = test_storage();
        storage.encryption.as_mut().unwrap().volumes[0].target_id = "mnt".to_owned();
        storage.raid.software[0].devices = Vec::new();
        storage.validate().unwrap_err();
    }

    /// Encrypted volume target ID must not be a software RAID array of A/B update volume pairs.
    #[test]
    fn test_validate_encryption_target_id_raid_ab_update_volume_pair_fail() {
        let mut storage: Storage = test_storage();
        storage.encryption.as_mut().unwrap().volumes[0].target_id = "mnt".to_owned();
        storage.raid.software[0].devices = vec!["root".to_owned()];
        // Remove the first mount point
        storage.validate().unwrap_err();
    }

    /// Encrypted volume target ID must not be a disk
    #[test]
    fn test_validate_encryption_target_id_disk_fail() {
        let mut storage: Storage = test_storage();
        storage.encryption.as_mut().unwrap().volumes[0].target_id = "disk1".to_owned();
        storage.validate().unwrap_err();
    }

    /// Encrypted volume target ID can be a software RAID array instead of a partition
    #[test]
    fn test_validate_encryption_target_id_raid_pass() {
        let mut storage: Storage = test_storage();
        storage.encryption.as_mut().unwrap().volumes[0].target_id = "mnt".to_owned();
        storage.mount_points.remove(2);
        storage.validate().unwrap();
    }

    /// Encrypted volume target ID must not be an A/B update volume pair
    #[test]
    fn test_validate_encryption_target_id_ab_update_volume_pair_fail() {
        let mut storage: Storage = test_storage();
        storage.encryption.as_mut().unwrap().volumes[0].target_id = "root".to_owned();
        storage.images.remove(1);
        storage.mount_points.remove(0);
        storage.validate().unwrap_err();
    }

    /// Encrypted volume target IDs must be unique
    #[test]
    fn test_validate_encryption_target_id_duplicate_fail() {
        let mut storage: Storage = test_storage();
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
        storage.validate().unwrap_err();
    }

    /// Encrypted volumes cannot target the same partition as a mount point
    #[test]
    fn test_validate_encryption_mount_point_target_part_id_equal_fail() {
        let mut storage: Storage = test_storage();
        storage.encryption.as_mut().unwrap().volumes[0].target_id = "mnt-raid-1".to_owned();
        storage.mount_points[2].target_id = "mnt-raid-1".to_owned();
        storage.validate().unwrap_err();
    }

    /// Encrypted volumes cannot target the same partition as a software RAID array
    #[test]
    fn test_validate_encryption_software_raid_target_part_id_equal_fail() {
        let mut storage: Storage = test_storage();
        storage.encryption.as_mut().unwrap().volumes[0].target_id = "mnt-raid-1".to_owned();
        storage.validate().unwrap_err();
    }

    /// Encrypted volumes cannot target the same partition as A/B update volume pair (A)
    #[test]
    fn test_validate_encryption_ab_update_volume_pair_a_part_id_equal_fail() {
        let mut storage: Storage = test_storage();
        storage.encryption.as_mut().unwrap().volumes[0].target_id = "root-a".to_owned();
        storage.disks[1].partitions[1].partition_type = PartitionType::LinuxGeneric;
        storage.disks[1].partitions[3].partition_type = PartitionType::LinuxGeneric;
        storage.validate().unwrap_err();
    }

    /// Encrypted volumes cannot target the same partition as A/B update volume pair (B)
    #[test]
    fn test_validate_encryption_ab_update_volume_pair_b_part_id_equal_fail() {
        let mut storage: Storage = test_storage();
        storage.encryption.as_mut().unwrap().volumes[0].target_id = "root-b".to_owned();
        storage.disks[1].partitions[1].partition_type = PartitionType::LinuxGeneric;
        storage.disks[1].partitions[3].partition_type = PartitionType::LinuxGeneric;
        storage.validate().unwrap_err();
    }

    /// Encrypted volumes cannot target the same software RAID array as an A/B update volume pair (A)
    #[test]
    fn test_validate_encryption_ab_update_volume_pair_a_raid_id_equal_fail() {
        let mut storage: Storage = test_storage();
        storage.encryption.as_mut().unwrap().volumes[0].target_id = "mnt".to_owned();
        storage.mount_points.remove(2); // remove /mnt mount point
        storage.ab_update.as_mut().unwrap().volume_pairs[0].volume_a_id = "mnt".to_owned();
        storage.validate().unwrap_err();
    }

    /// Encrypted volumes cannot target the same software RAID array as an A/B update volume pair (B)
    #[test]
    fn test_validate_encryption_ab_update_volume_pair_b_raid_id_equal_fail() {
        let mut storage: Storage = test_storage();
        storage.encryption.as_mut().unwrap().volumes[0].target_id = "mnt".to_owned();
        storage.mount_points.remove(2); // remove /mnt mount point
        storage.ab_update.as_mut().unwrap().volume_pairs[0].volume_b_id = "mnt".to_owned();
        storage.validate().unwrap_err();
    }

    /// Encrypted volumes cannot target the same partition as an image
    #[test]
    fn test_validate_encryption_image_target_part_id_equal_fail() {
        let mut storage: Storage = test_storage();
        storage.images[0].target_id = "srv-enc".to_owned();
        storage.validate().unwrap_err();
    }

    /// Image must be an A/B update volume pair if format is raw-lzma
    #[test]
    #[cfg(feature = "sysupdate")]
    fn test_validate_image_raw_lzma_ab_update_volume_pair_pass() {
        let mut storage: Storage = test_storage();
        storage.images[1].format = ImageFormat::RawLzma;
        storage.validate().unwrap();
    }

    /// Image must not be a partition if format is ram-lzma
    #[test]
    #[cfg(feature = "sysupdate")]
    fn test_validate_image_raw_lzma_partition_fail() {
        let mut storage: Storage = test_storage();
        storage.images[0].format = ImageFormat::RawLzma;
        storage.validate().unwrap_err();
    }

    /// Images can target encrypted volumes
    #[test]
    fn test_validate_image_target_id_encryption_pass() {
        let mut storage: Storage = test_storage();
        storage.images[0].target_id = "srv".to_owned();
        storage.validate().unwrap();
    }
}
