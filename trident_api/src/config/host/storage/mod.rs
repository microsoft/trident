use std::{collections::HashSet, path::PathBuf};

use anyhow::{bail, ensure, Context, Error};
use serde::{Deserialize, Serialize};
use strum_macros::{Display, EnumString};

use crate::{constants::SWAP_FILESYSTEM, is_default, BlockDeviceId};

use imaging::{AbUpdate, Image};

#[cfg(feature = "schemars")]
use schemars::JsonSchema;

#[cfg(feature = "schemars")]
use crate::schema_helpers::{block_device_id_list_schema, block_device_id_schema};

pub mod imaging;
mod serde_size;

/// Storage configuration describes the disks of the host that will be used to
/// store the OS and data. Not all disks of the host need to be captured inside
/// the Host Configuration, only those that Trident should operate on.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct Storage {
    /// A list of disks that will be used for the host.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disks: Vec<Disk>,

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
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
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

/// Per partition configuration.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct Partition {
    /// A unique identifier for the partition.
    ///
    /// This is a user defined string that allows to link the partition to the
    /// mount points and also to results in the Host Status. The identifier
    /// needs to be unique across all types of devices, not just partitions.
    #[cfg_attr(feature = "schemars", schemars(schema_with = "block_device_id_schema"))]
    pub id: BlockDeviceId,

    /// The type of the partition.
    ///
    /// As defined by the [Discoverable Partitions Specification](https://uapi-group.org/specifications/specs/discoverable_partitions_specification/).
    #[serde(rename = "type")]
    pub partition_type: PartitionType,

    /// Size of the partition.
    ///
    /// Format: String `<number>[<unit>]`
    ///
    /// Accepted values:
    ///
    /// - `grow`: Use all available space.
    ///
    /// - A number with optional unit suffixes: K, M, G, T (to the base of 1024),
    ///   bytes by default when no unit is specified.
    ///
    /// Examples:
    ///
    /// - `1G`
    ///
    /// - `200M`
    ///
    /// - `grow`
    #[cfg_attr(feature = "schemars", schemars(with = "String"))]
    pub size: PartitionSize,
}

/// Partition size enum.
/// Serialize and Deserialize traits are implemented manually in the crate::serde module.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub enum PartitionSize {
    /// # Grow
    ///
    /// Grow a partition to use all available space.
    ///
    /// String equivalent is defined in constants::PARTITION_SIZE_GROW
    Grow,

    /// # Fixed
    ///
    /// Fixed size in bytes.
    Fixed(u64),
    // Not implemented yet but left as a reference for the future.
    // Min(u64),
    // Max(u64),
    // MinMax(u64, u64),
}

/// Partition types as defined by The Discoverable Partitions Specification (https://uapi-group.org/specifications/specs/discoverable_partitions_specification/).
#[derive(Serialize, Deserialize, Copy, Clone, Debug, Hash, Eq, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub enum PartitionType {
    /// # EFI System Partition
    ///
    /// `C12A7328-F81F-11D2-BA4B-00A0C93EC93B`
    Esp,

    /// # Root partition
    ///
    /// x64: `4F68BCE3-E8CD-4DB1-96E7-FBCAF984B709`
    Root,

    /// # Swap partition
    ///
    /// `0657fd6d-a4ab-43c4-84e5-0933c84b4f4f`
    Swap,

    /// # Root partition with dm-verity enabled
    ///
    /// x64: `2c7357ed-ebd2-46d9-aec1-23d437ec2bf5`
    RootVerity,

    /// # Home partition
    ///
    /// `933ac7e1-2eb4-4f13-b844-0e14e2aef915`
    Home,

    /// # Var partition
    ///
    /// `4d21b016-b534-45c2-a9fb-5c16e091fd2d`
    Var,

    /// # Usr partition
    ///
    /// x64: `8484680c-9521-48c6-9c11-b0720656f69e`
    Usr,

    /// # Tmp partition
    ///
    /// `7ec6f557-3bc5-4aca-b293-16ef5df639d1`
    Tmp,

    /// # Generic Linux partition
    ///
    /// `0fc63daf-8483-4772-8e79-3d69d8477de4`
    LinuxGeneric,

    /// # Server Data partition
    ///
    /// `3b8f8425-20e0-4f3b-907f-1a25a76f98e8`
    ///
    /// To use this partition type on the disk with the root volume, make sure
    /// to not have `/srv` symlink present in your root volume filesystem. If
    /// you do, remove it before running Trident (e.g. by using MIC).
    Srv,
}

impl PartitionType {
    /// Helper function that returns PartititionType as a string. Return values
    /// are based on GPT partition type identifiers, as defined in the Type
    /// section of systemd repart.d manual:
    /// https://www.man7.org/linux/man-pages/man5/repart.d.5.html.
    pub fn to_sdrepart_part_type(&self) -> &str {
        match self {
            PartitionType::Esp => "esp",
            PartitionType::Root => "root",
            PartitionType::Swap => "swap",
            PartitionType::RootVerity => "root-verity",
            PartitionType::Home => "home",
            PartitionType::Var => "var",
            PartitionType::Usr => "usr",
            PartitionType::Tmp => "tmp",
            PartitionType::LinuxGeneric => "linux-generic",
            PartitionType::Srv => "srv",
        }
    }
}

/// RAID configuration for a host.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
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
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
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
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
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

    /// The ID of the partition that will be mounted at this mount point.
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

    pub fn validate(&self) -> Result<(), Error> {
        let mut block_device_ids = Vec::new();
        let mut partitions = HashSet::new();
        let mut raid_arrays = HashSet::new();
        let mut volume_pairs = HashSet::new();

        // Collect lists of all block device ids
        for disk in &self.disks {
            block_device_ids.push(disk.id.clone());
            for partition in &disk.partitions {
                block_device_ids.push(partition.id.clone());
                partitions.insert(partition.id.clone());
            }
        }
        for raid in &self.raid.software {
            block_device_ids.push(raid.id.clone());
            raid_arrays.insert(raid.id.clone());
        }
        if let Some(ab_update) = &self.ab_update {
            for pair in &ab_update.volume_pairs {
                block_device_ids.push(pair.id.clone());
                volume_pairs.insert(pair.id.clone());
            }
        }

        // Check for duplicates
        let mut block_device_ids_set = HashSet::new();
        for id in &block_device_ids {
            if !block_device_ids_set.insert(id.clone()) {
                bail!("Block device ID {id} is used more than once");
            }
        }

        // Check that all references are valid
        if let Some(ab_update) = &self.ab_update {
            for pair in &ab_update.volume_pairs {
                for volume in [&pair.volume_a_id, &pair.volume_b_id] {
                    ensure!(
                        block_device_ids_set.contains(volume),
                        "Block device ID {id} is used in the A/B update configuration but is not defined",
                        id = volume
                    );
                    ensure!(
                        partitions.contains(volume) || raid_arrays.contains(volume),
                        "Block device ID {id} is used in the A/B update configuration but is not a partition or raid array",
                        id = volume
                    );
                }
            }
        }
        for image in &self.images {
            ensure!(
                block_device_ids_set.contains(&image.target_id),
                "Block device ID {id} is used in the image configuration but is not defined in the Storage configuration",
                id = image.target_id
            );
            ensure!(
                !raid_arrays.contains(&image.target_id),
                "Image targets a RAID array '{id}', which is not supported",
                id = image.target_id,
            );
            ensure!(
                partitions.contains(&image.target_id) || volume_pairs.contains(&image.target_id),
                "Block device ID {id} is used in the image configuration but is not a partition or A/B update volume pair",
                id = image.target_id
            );
        }
        for mount_point in &self.mount_points {
            ensure!(
                block_device_ids_set.contains(&mount_point.target_id),
                "Block device ID {id} is used in the mount point configuration but is not defined in the Storage configuration",
                id = mount_point.target_id
            );
            ensure!(
                partitions.contains(&mount_point.target_id) || raid_arrays.contains(&mount_point.target_id) || volume_pairs.contains(&mount_point.target_id),
                "Block device ID {id} is used in the mount point configuration but is not a partition, raid array, or volume pair",
                id = mount_point.target_id
            );
        }
        for raid in &self.raid.software {
            for device in &raid.devices {
                ensure!(
                    block_device_ids_set.contains(device),
                    "Block device ID {device} is used in the RAID configuration but is not defined in the Storage configuration",
                );
                ensure!(
                    partitions.contains(device),
                    "Block device ID {device} is used in the RAID configuration but is not a partition",
                );
            }
        }

        // Ensure mutual exclusivity
        if let Some(ab_update) = &self.ab_update {
            for pair in &ab_update.volume_pairs {
                ensure!(
                    pair.volume_a_id != pair.volume_b_id,
                    "A/B update volume pair {id} has the same volume ID for both volumes",
                    id = pair.id
                );
            }
        }

        // Check that devices are valid partitions and only part of a single RAID array
        let mut raid_devices = HashSet::new();
        for software_raid_config in &self.raid.software {
            let mut device_sizes = Vec::<PartitionSize>::new();
            for device_id in &software_raid_config.devices {
                if !raid_devices.insert(device_id.clone()) {
                    bail!("Block device '{device_id}' cannot be part of multiple RAID arrays");
                }

                let partition = self.get_partition(device_id)
                    .context(format!("Device id '{device_id}' was set as dependency of a RAID array, but is not a valid partition"))?;
                device_sizes.push(partition.size.clone());
            }
            ensure!(
                device_sizes.iter().min() == device_sizes.iter().max(),
                "RAID array {} has underlying devices with different sizes",
                software_raid_config.id
            );
        }

        // Test for expected mount point configurations
        for mount_point in &self.mount_points {
            ensure!(
                mount_point.path.starts_with("/") || mount_point.filesystem == SWAP_FILESYSTEM,
                "Mount point path must be absolute or the filesystem has to be 'swap'"
            );
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use imaging::{AbVolumePair, ImageFormat};

    use super::*;

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
                sha256: "ignored".to_string(),
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
                target_id: "part1".to_owned(),
                path: PathBuf::from("/"),
            }],
            images: vec![Image {
                target_id: "part1".to_owned(),
                url: "".to_owned(),
                sha256: "".to_owned(),
                format: ImageFormat::RawZstd,
            }],
            ab_update: Some(AbUpdate {
                volume_pairs: vec![AbVolumePair {
                    id: "ab1".to_owned(),
                    volume_a_id: "part1".to_owned(),
                    volume_b_id: "part2".to_owned(),
                }],
            }),
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
}
