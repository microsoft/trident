use std::{collections::HashSet, path::PathBuf};

use anyhow::{anyhow, bail, ensure, Context, Error};
use serde::{Deserialize, Serialize};
use strum_macros::{Display, EnumString};
use url::Url;

use crate::{constants::SWAP_FILESYSTEM, is_default, BlockDeviceId};

use imaging::{AbUpdate, Image, ImageFormat};

#[cfg(feature = "schemars")]
use schemars::JsonSchema;

#[cfg(feature = "schemars")]
use crate::schema_helpers::{block_device_id_list_schema, block_device_id_schema};

pub mod imaging;
pub mod partitions;
mod serde_hash;

use partitions::{AdoptedPartition, Partition, PartitionSize, PartitionType};

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
    pub recovery_key_url: Option<String>,

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

    pub fn validate(&self) -> Result<(), Error> {
        let mut block_device_ids = Vec::new();
        let mut partitions = HashSet::new();
        let mut raid_arrays = HashSet::new();
        let mut ab_volume_pairs = HashSet::new();
        let mut encrypted_volumes = HashSet::new();

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
                ab_volume_pairs.insert(pair.id.clone());
            }
        }
        if let Some(encryption) = &self.encryption {
            for volume in &encryption.volumes {
                block_device_ids.push(volume.id.clone());
                ensure!(
                    encrypted_volumes.insert(volume.id.clone()),
                    "ID '{id}' is used by multiple encrypted volumes",
                    id = volume.id
                );
            }
        }

        // Check for duplicates
        let mut block_device_ids_set = HashSet::new();
        for id in &block_device_ids {
            if !block_device_ids_set.insert(id.clone()) {
                bail!("Block device ID '{id}' is used more than once");
            }
        }

        for raid in &self.raid.software {
            ensure!(
                !raid.devices.is_empty(),
                "Software RAID array '{id}' has no devices",
                id = raid.id
            );
            for device in &raid.devices {
                ensure!(
                    partitions.contains(device),
                    "Block device ID '{device}' is used in the RAID configuration but is not a partition",
                );
            }
        }

        if let Some(encryption) = &self.encryption {
            let mut encryption_target_ids_set: HashSet<&BlockDeviceId> = HashSet::new();
            let mut encryption_device_names_set: HashSet<&String> = HashSet::new();

            for volume in &encryption.volumes {
                // Encrypted volume target IDs must be unique
                if !encryption_target_ids_set.insert(&volume.target_id) {
                    bail!(
                        "Target ID '{tid}' is used by multiple encrypted volumes",
                        tid = volume.target_id
                    );
                }

                // Encrypted volume device names must be unique
                ensure!(
                    encryption_device_names_set.insert(&volume.device_name),
                    "Encrypted volume device name '{name}' is used more than once",
                    name = volume.device_name
                );

                // Encrypted volumes cannot target the same block device
                // id as a mount point
                for mount_point in &self.mount_points {
                    ensure!(
                        volume.target_id != mount_point.target_id,
                        "Target ID '{tid}' of encrypted volume '{vid}' is already used by mount point '{mp}' ('{fs}')",
                        tid = volume.target_id,
                        vid = volume.id,
                        mp = mount_point.path.display(),
                        fs = mount_point.filesystem
                    );
                }

                // Encrypted volumes cannot target the same block device
                // id as a software RAID array
                for array in &self.raid.software {
                    for device in &array.devices {
                        ensure!(
                            volume.target_id != *device,
                            "Target ID '{tid}' of encrypted volume '{vid}' is already used by software RAID array '{aid}'",
                            tid = volume.target_id,
                            vid = volume.id,
                            aid = array.id
                        );
                    }
                }

                // Encrypted volumes cannot target the same block device
                // id as an A/B update volume pair
                if let Some(ab_update) = &self.ab_update {
                    for volume_pair in &ab_update.volume_pairs {
                        ensure!(
                            volume.target_id != volume_pair.volume_a_id,
                            "Target ID '{tid}' of encrypted volume '{vid}' is already used by A/B update volume pair '{abid}' (A)",
                            tid = volume.target_id,
                            vid = volume.id,
                            abid = volume_pair.id
                        );
                        ensure!(
                            volume.target_id != volume_pair.volume_b_id,
                            "Target ID '{tid}' of encrypted volume '{vid}' is already used by A/B update volume pair '{abid}' (B)",
                            tid = volume.target_id,
                            vid = volume.id,
                            abid = volume_pair.id
                        );
                    }
                }

                // Encrypted volumes cannot target the same block device
                // id as an image
                for image in &self.images {
                    ensure!(
                        volume.target_id != image.target_id,
                        "Target ID '{tid}' of encrypted volume '{vid}' is already used by image '{img}'",
                        tid = volume.target_id,
                        vid = volume.id,
                        img = image.url
                    );
                }

                // Encrypted volume target IDs must not be an esp, root,
                // or root-verity, or a software RAID array of such
                // partitions.
                let pty = if partitions.contains(&volume.target_id) {
                    // This should find a partition because we already
                    // checked that the target ID is in partitions.
                    self.disks
                        .iter()
                        .flat_map(|disk| &disk.partitions)
                        .find(|partition| partition.id == volume.target_id)
                        .ok_or(anyhow!("Partition '{}' not found", volume.target_id))?
                        .partition_type
                } else {
                    ensure!(
                            raid_arrays.contains(&volume.target_id),
                            "Target ID '{tid}' of encrypted volume '{vid}' is not a partition id or software RAID array",
                            tid = volume.target_id,
                            vid = volume.id
                        );

                    // This should find a software RAID array because we
                    // already checked that the target ID is in
                    // raid_arrays.
                    //
                    // Further, this should find a first device because we
                    // already checked that all RAID arrays have at least
                    // one device.
                    let partition_id = self
                        .raid
                        .software
                        .iter()
                        .find(|array| array.id == volume.target_id)
                        .ok_or(anyhow!(
                            "Software RAID array '{}' not found",
                            volume.target_id
                        ))?
                        .devices
                        .first()
                        .ok_or(anyhow!(
                            "Software RAID array '{}' has no devices",
                            volume.target_id
                        ))?;

                    // Similarly, this should find a partition because we
                    // already checked that the devices of all software
                    // RAID arrays are in partitions.
                    self.disks
                        .iter()
                        .flat_map(|disk| &disk.partitions)
                        .find(|partition| partition.id == *partition_id)
                        .ok_or(anyhow!("Partition '{}' not found", partition_id))?
                        .partition_type
                };
                ensure!(
                    pty != PartitionType::Esp && pty != PartitionType::Root && pty != PartitionType::RootVerity,
                    "Target ID '{tid}' of encrypted volume '{vid}' has an invalid partition type '{pty}'",
                    tid = volume.target_id,
                    vid = volume.id,
                    pty = pty.to_sdrepart_part_type()
                );
            }

            // Encryption recovery key URLs must start with file://
            if let Some(recovery_key_url) = &encryption.recovery_key_url {
                let recovery_key_url = Url::parse(recovery_key_url).context(format!(
                    "Encryption recovery key '{}' is not a URL",
                    recovery_key_url
                ))?;
                ensure!(
                    recovery_key_url.scheme() == "file",
                    "Encryption recovery key URL '{}' has an invalid scheme '{}'",
                    recovery_key_url,
                    recovery_key_url.scheme()
                );
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
                        partitions.contains(volume) || raid_arrays.contains(volume) || encrypted_volumes.contains(volume),
                        "Block device ID {id} is used in the A/B update configuration but is not a partition, software RAID array, or encrypted volume",
                        id = volume
                    );
                }
            }
        }
        for image in &self.images {
            ensure!(
                block_device_ids_set.contains(&image.target_id),
                "Block device ID '{id}' is used in the image configuration but is not defined in the Storage configuration",
                id = image.target_id
            );
            ensure!(
                !raid_arrays.contains(&image.target_id),
                "Image targets a RAID array '{id}', which is not supported",
                id = image.target_id,
            );
            ensure!(
                partitions.contains(&image.target_id) || encrypted_volumes.contains(&image.target_id) || ab_volume_pairs.contains(&image.target_id),
                "Block device ID '{id}' is used in the image configuration but is not a partition, encrypted volume, or A/B update volume pair",
                id = image.target_id
            );

            if image.format == ImageFormat::RawLzma && !ab_volume_pairs.contains(&image.target_id) {
                bail!(
                    "Image '{url}' is raw-lzma but block device ID '{tid}' is not an A/B update volume pair",
                    url = image.url,
                    tid = image.target_id
                );
            }
        }
        for mount_point in &self.mount_points {
            ensure!(
                block_device_ids_set.contains(&mount_point.target_id),
                "Block device ID '{id}' is used in the mount point configuration but is not defined in the Storage configuration",
                id = mount_point.target_id
            );
            ensure!(
                partitions.contains(&mount_point.target_id) || raid_arrays.contains(&mount_point.target_id) || encrypted_volumes.contains(&mount_point.target_id) || ab_volume_pairs.contains(&mount_point.target_id),
                "Block device ID {id} is used in the mount point configuration but is not a partition, raid array, encrypted volume, or volume pair",
                id = mount_point.target_id
            );
        }

        // Ensure mutual exclusivity
        if let Some(ab_update) = &self.ab_update {
            for pair in &ab_update.volume_pairs {
                ensure!(
                    pair.volume_a_id != pair.volume_b_id,
                    "A/B update volume pair '{id}' has the same volume ID for both volumes",
                    id = pair.id
                );
            }
        }

        // Check that devices are valid partitions and only part of a single RAID array
        let mut raid_devices = HashSet::new();
        for software_raid_config in &self.raid.software {
            for device_id in &software_raid_config.devices {
                if !raid_devices.insert(device_id.clone()) {
                    bail!("Block device '{device_id}' cannot be part of multiple RAID arrays");
                }
            }

            // Get sizes of all devices in the RAID array:
            // 1. Check that all devices are partitions
            // 2. Check that all partitions have fixed sizes
            // 3. Collect results
            let device_sizes: Vec<u64> = software_raid_config
                .devices
                .iter()
                .map(|device_id| {
                    let partition = self.get_partition(device_id)
                        .context(format!("Device id '{device_id}' was set as dependency of a RAID array, but is not a valid partition"))?;
                    if let PartitionSize::Fixed(size) = partition.size {
                        Ok(size)
                    } else {
                        bail!("RAID array references partition '{device_id}' with a non-fixed size")
                    }
                })
                .collect::<Result<Vec<u64>, Error>>()
                .context(format!("RAID array '{}' has invalid members", software_raid_config.id))?;

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

    use imaging::{AbVolumePair, ImageFormat, ImageSha256};

    use super::*;

    macro_rules! TEST_STORAGE {
        () => {
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
                    recovery_key_url: Some("file:///recovery.key".to_owned()),
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
        };
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
                target_id: "part1".to_owned(),
                path: PathBuf::from("/"),
            }],
            images: vec![Image {
                target_id: "part1".to_owned(),
                url: "".to_owned(),
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
        let storage: Storage = TEST_STORAGE!();
        storage.validate().unwrap();
    }

    // A/B update volume pairs can target encrypted volumes (A)
    #[test]
    fn test_validate_ab_update_volume_pair_a_id_encryption_pass() {
        let mut storage: Storage = TEST_STORAGE!();
        storage.ab_update.as_mut().unwrap().volume_pairs[0].volume_a_id = "srv".to_owned();
        storage.validate().unwrap();
    }

    // A/B update volume pairs can target encrypted volumes (B)
    #[test]
    fn test_validate_ab_update_volume_pair_b_id_encryption_pass() {
        let mut storage: Storage = TEST_STORAGE!();
        storage.ab_update.as_mut().unwrap().volume_pairs[0].volume_b_id = "srv".to_owned();
        storage.validate().unwrap();
    }

    // Software RAID arrays must have one or more devices
    #[test]
    fn test_validate_software_raid_array_no_devices_fail() {
        let mut storage: Storage = TEST_STORAGE!();
        storage.raid.software[0].devices = Vec::new();
        assert_eq!(
            storage.validate().unwrap_err().to_string(),
            "Software RAID array 'mnt' has no devices"
        );
    }

    // Software RAID arrays cannot target encrypted volumes
    #[test]
    fn test_validate_software_raid_target_id_encryption_fail() {
        let mut storage: Storage = TEST_STORAGE!();
        storage.raid.software[0].devices[0] = "srv".to_owned();
        assert_eq!(
            storage.validate().unwrap_err().to_string(),
            "Block device ID 'srv' is used in the RAID configuration but is not a partition"
        );
    }

    // Encrypted volumes and disks must not share the same id
    #[test]
    fn test_validate_encryption_disks_share_id_fail() {
        let mut storage: Storage = TEST_STORAGE!();
        storage.encryption.as_mut().unwrap().volumes[0].id = "disk1".to_owned();
        assert_eq!(
            storage.validate().unwrap_err().to_string(),
            "Block device ID 'disk1' is used more than once"
        );
    }

    // Encrypted volumes and partitions must not share the same id
    #[test]
    fn test_validate_encryption_partitions_share_id_fail() {
        let mut storage: Storage = TEST_STORAGE!();
        storage.encryption.as_mut().unwrap().volumes[0].id = "esp".to_owned();
        assert_eq!(
            storage.validate().unwrap_err().to_string(),
            "Block device ID 'esp' is used more than once"
        );
    }

    // Encrypted volumes and software RAID arrays must not share the same id
    #[test]
    fn test_validate_encryption_raid_arrays_share_id_fail() {
        let mut storage: Storage = TEST_STORAGE!();
        storage.encryption.as_mut().unwrap().volumes[0].id = "mnt".to_owned();
        assert_eq!(
            storage.validate().unwrap_err().to_string(),
            "Block device ID 'mnt' is used more than once"
        );
    }

    // Encrypted volumes and A/B update volume pairs must not share the same id
    #[test]
    fn test_validate_encryption_ab_update_volume_pairs_share_id_fail() {
        let mut storage: Storage = TEST_STORAGE!();
        storage.encryption.as_mut().unwrap().volumes[0].id = "root".to_owned();
        assert_eq!(
            storage.validate().unwrap_err().to_string(),
            "Block device ID 'root' is used more than once"
        );
    }

    // Encrypted volumes themselves must not share the same id
    #[test]
    fn test_validate_encryption_volumes_share_id_fail() {
        let mut storage: Storage = TEST_STORAGE!();
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
            storage.validate().unwrap_err().to_string(),
            "ID 'srv' is used by multiple encrypted volumes"
        );
    }

    // Encrypted volume device names must be unique
    #[test]
    fn test_validate_encryption_device_names_duplicate_fail() {
        let mut storage: Storage = TEST_STORAGE!();
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
            storage.validate().unwrap_err().to_string(),
            "Encrypted volume device name 'luks-srv' is used more than once"
        );
    }

    // Encryption recovery key must be a URL
    #[test]
    fn test_validate_encryption_recovery_key_not_url_fail() {
        let mut storage: Storage = TEST_STORAGE!();
        storage.encryption.as_mut().unwrap().recovery_key_url =
            Some("/path/to/recovery.key".to_owned());
        assert_eq!(
            storage.validate().unwrap_err().to_string(),
            "Encryption recovery key '/path/to/recovery.key' is not a URL"
        );
    }

    // Encryption recovery key may have file scheme
    #[test]
    fn test_validate_encryption_recovery_key_file_scheme_pass() {
        let mut storage: Storage = TEST_STORAGE!();
        storage.encryption.as_mut().unwrap().recovery_key_url =
            Some("file:///path/to/recovery.key".to_owned());
        storage.validate().unwrap();
    }

    // Encryption recovery key must not have https scheme
    #[test]
    fn test_validate_encryption_recovery_key_http_scheme_fail() {
        let mut storage: Storage = TEST_STORAGE!();
        storage.encryption.as_mut().unwrap().recovery_key_url =
            Some("https://www.example.com/recovery.key".to_owned());
        assert_eq!(
            storage.validate().unwrap_err().to_string(),
            "Encryption recovery key URL 'https://www.example.com/recovery.key' has an invalid scheme 'https'"
        );
    }

    // Encrypted volume target ID may be a home partition
    #[test]
    fn test_validate_encryption_target_id_home_pass() {
        let mut storage: Storage = TEST_STORAGE!();
        storage.disks[1].partitions[5].partition_type = PartitionType::Home;
        storage.validate().unwrap();
    }

    // Encrypted volume target ID must not be an esp partition
    #[test]
    fn test_validate_encryption_target_id_esp_fail() {
        let mut storage: Storage = TEST_STORAGE!();
        storage.encryption.as_mut().unwrap().volumes[0].target_id = "esp".to_owned();
        storage.mount_points.remove(1);
        storage.images.remove(0);
        assert_eq!(
            storage.validate().unwrap_err().to_string(),
            "Target ID 'esp' of encrypted volume 'srv' has an invalid partition type 'esp'"
        );
    }

    // Encrypted volume target ID must not be a root partition
    #[test]
    fn test_validate_encryption_target_id_root_fail() {
        let mut storage: Storage = TEST_STORAGE!();
        storage.encryption.as_mut().unwrap().volumes[0].target_id = "root-a".to_owned();
        storage.ab_update.as_mut().unwrap().volume_pairs[0].volume_a_id =
            "root-a-verity".to_owned();
        assert_eq!(
            storage.validate().unwrap_err().to_string(),
            "Target ID 'root-a' of encrypted volume 'srv' has an invalid partition type 'root'"
        );
    }

    // Encrypted volume target ID must not be a root-verity partition
    #[test]
    fn test_validate_encryption_target_id_root_verity_fail() {
        let mut storage: Storage = TEST_STORAGE!();
        storage
            .encryption
            .as_mut()
            .unwrap()
            .volumes
            .get_mut(0)
            .unwrap()
            .target_id = "root-a-verity".to_owned();
        assert_eq!(
            storage.validate().unwrap_err().to_string(),
            "Target ID 'root-a-verity' of encrypted volume 'srv' has an invalid partition type 'root-verity'"
        );
    }

    // Encrypted volume target ID may be a software RAID array of home partitions
    #[test]
    fn test_validate_encryption_target_id_raid_home_pass() {
        let mut storage: Storage = TEST_STORAGE!();
        storage.encryption.as_mut().unwrap().volumes[0].target_id = "mnt".to_owned();
        storage.mount_points.remove(2);
        storage.disks[1].partitions[3].partition_type = PartitionType::Home;
        storage.disks[1].partitions[4].partition_type = PartitionType::Home;
        storage.validate().unwrap();
    }

    // Encrypted volume target ID must not be a software RAID array of esp partitions
    #[test]
    fn test_validate_encryption_target_id_raid_esp_fail() {
        let mut storage: Storage = TEST_STORAGE!();
        storage.encryption.as_mut().unwrap().volumes[0].target_id = "mnt".to_owned();
        storage.mount_points.remove(2);
        storage.disks[1].partitions[3].partition_type = PartitionType::Esp;
        storage.disks[1].partitions[4].partition_type = PartitionType::Esp;
        storage.validate().unwrap();
    }

    // Encrypted volume target ID must not be a software RAID array of root partitions
    #[test]
    fn test_validate_encryption_target_id_raid_root_fail() {
        let mut storage: Storage = TEST_STORAGE!();
        storage.encryption.as_mut().unwrap().volumes[0].target_id = "mnt".to_owned();
        storage.mount_points.remove(2);
        storage.disks[1].partitions[3].partition_type = PartitionType::Root;
        storage.disks[1].partitions[4].partition_type = PartitionType::Root;
        storage.validate().unwrap();
    }

    // Encrypted volume target ID must not be a software RAID array of root-verity partitions
    #[test]
    fn test_validate_encryption_target_id_raid_root_verity_fail() {
        let mut storage: Storage = TEST_STORAGE!();
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

    // Encrypted volume target ID must not be a software RAID array of no devices.
    #[test]
    fn test_validate_encryption_target_id_raid_no_devices_fail() {
        let mut storage: Storage = TEST_STORAGE!();
        storage.encryption.as_mut().unwrap().volumes[0].target_id = "mnt".to_owned();
        storage.raid.software[0].devices = Vec::new();
        assert_eq!(
            storage.validate().unwrap_err().to_string(),
            "Software RAID array 'mnt' has no devices"
        );
    }

    // Encrypted volume target ID must not be a software RAID array of A/B update volume pairs.
    #[test]
    fn test_validate_encryption_target_id_raid_ab_update_volume_pair_fail() {
        let mut storage: Storage = TEST_STORAGE!();
        storage.encryption.as_mut().unwrap().volumes[0].target_id = "mnt".to_owned();
        storage.raid.software[0].devices = vec!["root".to_owned()];
        // Remove the first mount point
        assert_eq!(
            storage.validate().unwrap_err().to_string(),
            "Block device ID 'root' is used in the RAID configuration but is not a partition"
        );
    }

    // Encrypted volume target ID must not be a disk
    #[test]
    fn test_validate_encryption_target_id_disk_fail() {
        let mut storage: Storage = TEST_STORAGE!();
        storage.encryption.as_mut().unwrap().volumes[0].target_id = "disk1".to_owned();
        assert_eq!(
            storage.validate().unwrap_err().to_string(),
            "Target ID 'disk1' of encrypted volume 'srv' is not a partition id or software RAID array"
        );
    }

    // Encrypted volume target ID can be a software RAID array instead of a partition
    #[test]
    fn test_validate_encryption_target_id_raid_pass() {
        let mut storage: Storage = TEST_STORAGE!();
        storage.encryption.as_mut().unwrap().volumes[0].target_id = "mnt".to_owned();
        storage.mount_points.remove(2);
        storage.validate().unwrap();
    }

    // Encrypted volume target ID must not be an A/B update volume pair
    #[test]
    fn test_validate_encryption_target_id_ab_update_volume_pair_fail() {
        let mut storage: Storage = TEST_STORAGE!();
        storage.encryption.as_mut().unwrap().volumes[0].target_id = "root".to_owned();
        storage.images.remove(1);
        storage.mount_points.remove(0);
        assert_eq!(
            storage.validate().unwrap_err().to_string(),
            "Target ID 'root' of encrypted volume 'srv' is not a partition id or software RAID array"
        );
    }

    // Encrypted volume target IDs must be unique
    #[test]
    fn test_validate_encryption_target_id_duplicate_fail() {
        let mut storage: Storage = TEST_STORAGE!();
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
            storage.validate().unwrap_err().to_string(),
            "Target ID 'srv-enc' is used by multiple encrypted volumes"
        );
    }

    // Encrypted volumes cannot target the same partition as a mount point
    #[test]
    fn test_validate_encryption_mount_point_target_part_id_equal_fail() {
        let mut storage: Storage = TEST_STORAGE!();
        storage.encryption.as_mut().unwrap().volumes[0].target_id = "mnt-raid-1".to_owned();
        storage.mount_points[2].target_id = "mnt-raid-1".to_owned();
        assert_eq!(
            storage.validate().unwrap_err().to_string(),
            "Target ID 'mnt-raid-1' of encrypted volume 'srv' is already used by mount point '/mnt' ('ext4')"
        );
    }

    // Encrypted volumes cannot target the same partition as a software RAID array
    #[test]
    fn test_validate_encryption_software_raid_target_part_id_equal_fail() {
        let mut storage: Storage = TEST_STORAGE!();
        storage.encryption.as_mut().unwrap().volumes[0].target_id = "mnt-raid-1".to_owned();
        assert_eq!(
            storage.validate().unwrap_err().to_string(),
            "Target ID 'mnt-raid-1' of encrypted volume 'srv' is already used by software RAID array 'mnt'"
        );
    }

    // Encrypted volumes cannot target the same partition as A/B update volume pair (A)
    #[test]
    fn test_validate_encryption_ab_update_volume_pair_a_part_id_equal_fail() {
        let mut storage: Storage = TEST_STORAGE!();
        storage.encryption.as_mut().unwrap().volumes[0].target_id = "root-a".to_owned();
        storage.disks[1].partitions[1].partition_type = PartitionType::LinuxGeneric;
        storage.disks[1].partitions[3].partition_type = PartitionType::LinuxGeneric;
        assert_eq!(
            storage.validate().unwrap_err().to_string(),
            "Target ID 'root-a' of encrypted volume 'srv' is already used by A/B update volume pair 'root' (A)"
        );
    }

    // Encrypted volumes cannot target the same partition as A/B update volume pair (B)
    #[test]
    fn test_validate_encryption_ab_update_volume_pair_b_part_id_equal_fail() {
        let mut storage: Storage = TEST_STORAGE!();
        storage.encryption.as_mut().unwrap().volumes[0].target_id = "root-b".to_owned();
        storage.disks[1].partitions[1].partition_type = PartitionType::LinuxGeneric;
        storage.disks[1].partitions[3].partition_type = PartitionType::LinuxGeneric;
        assert_eq!(
            storage.validate().unwrap_err().to_string(),
            "Target ID 'root-b' of encrypted volume 'srv' is already used by A/B update volume pair 'root' (B)"
        );
    }

    // Encrypted volumes cannot target the same software RAID array as an A/B update volume pair (A)
    #[test]
    fn test_validate_encryption_ab_update_volume_pair_a_raid_id_equal_fail() {
        let mut storage: Storage = TEST_STORAGE!();
        storage.encryption.as_mut().unwrap().volumes[0].target_id = "mnt".to_owned();
        storage.mount_points.remove(2); // remove /mnt mount point
        storage.ab_update.as_mut().unwrap().volume_pairs[0].volume_a_id = "mnt".to_owned();
        assert_eq!(
            storage.validate().unwrap_err().to_string(),
            "Target ID 'mnt' of encrypted volume 'srv' is already used by A/B update volume pair 'root' (A)"
        );
    }

    // Encrypted volumes cannot target the same software RAID array as an A/B update volume pair (B)
    #[test]
    fn test_validate_encryption_ab_update_volume_pair_b_raid_id_equal_fail() {
        let mut storage: Storage = TEST_STORAGE!();
        storage.encryption.as_mut().unwrap().volumes[0].target_id = "mnt".to_owned();
        storage.mount_points.remove(2); // remove /mnt mount point
        storage.ab_update.as_mut().unwrap().volume_pairs[0].volume_b_id = "mnt".to_owned();
        assert_eq!(
            storage.validate().unwrap_err().to_string(),
            "Target ID 'mnt' of encrypted volume 'srv' is already used by A/B update volume pair 'root' (B)"
        );
    }

    // Encrypted volumes cannot target the same partition as an image
    #[test]
    fn test_validate_encryption_image_target_part_id_equal_fail() {
        let mut storage: Storage = TEST_STORAGE!();
        storage.images[0].target_id = "srv-enc".to_owned();
        assert_eq!(
            storage.validate().unwrap_err().to_string(),
            "Target ID 'srv-enc' of encrypted volume 'srv' is already used by image 'file:///esp.raw.zst'"
        );
    }

    // Image must be an A/B update volume pair if format is raw-lzma
    #[test]
    fn test_validate_image_raw_lzma_ab_update_volume_pair_pass() {
        let mut storage: Storage = TEST_STORAGE!();
        storage.images[1].format = ImageFormat::RawLzma;
        storage.validate().unwrap();
    }

    // Image must not be a partition if format is ram-lzma
    #[test]
    fn test_validate_image_raw_lzma_partition_fail() {
        let mut storage: Storage = TEST_STORAGE!();
        storage.images[0].format = ImageFormat::RawLzma;
        assert_eq!(
            storage.validate().unwrap_err().to_string(),
            "Image 'file:///esp.raw.zst' is raw-lzma but block device ID 'esp' is not an A/B update volume pair"
        );
    }

    // Images can target encrypted volumes
    #[test]
    fn test_validate_image_target_id_encryption_pass() {
        let mut storage: Storage = TEST_STORAGE!();
        storage.images[0].target_id = "srv".to_owned();
        storage.validate().unwrap();
    }
}
