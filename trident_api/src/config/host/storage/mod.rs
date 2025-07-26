use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
};

use log::trace;
use serde::{Deserialize, Serialize};

#[cfg(feature = "schemars")]
use schemars::JsonSchema;
use swap::Swap;

use crate::{
    constants::{
        BOOT_MOUNT_POINT_PATH, ESP_MOUNT_POINT_PATH, MOUNT_OPTION_READ_ONLY, ROOT_MOUNT_POINT_PATH,
        ROOT_VERITY_DEVICE_NAME, TRIDENT_OVERLAY_PATH, USR_MOUNT_POINT_PATH,
        USR_VERITY_DEVICE_NAME, VAR_TMP_PATH,
    },
    is_default, BlockDeviceId,
};

use super::error::HostConfigurationStaticValidationError;

pub mod abupdate;
pub mod disks;
pub mod encryption;
pub mod filesystem;
pub mod filesystem_types;
pub mod partitions;
pub mod raid;
pub mod storage_graph;
pub mod swap;
pub mod verity;

use self::{
    abupdate::AbUpdate,
    disks::Disk,
    encryption::Encryption,
    filesystem::{FileSystem, MountPointInfo},
    partitions::Partition,
    raid::Raid,
    storage_graph::{
        builder::StorageGraphBuilder,
        error::StorageGraphBuildError,
        graph::{StorageGraph, VolumeStatus},
    },
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encryption: Option<Encryption>,

    /// RAID configuration.
    #[serde(default, skip_serializing_if = "is_default")]
    pub raid: Raid,

    /// A/B update configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ab_update: Option<AbUpdate>,

    /// Filesystems in this host.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filesystems: Vec<FileSystem>,

    /// Verity device configuration.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub verity: Vec<VerityDevice>,

    /// Swap device configuration.
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "crate::primitives::shortcuts::vec_string_or_struct"
    )]
    #[cfg_attr(
        feature = "schemars",
        schemars(
            schema_with = "crate::primitives::shortcuts::vec_string_or_struct_schema::<Swap>"
        )
    )]
    pub swap: Vec<Swap>,
}

impl Storage {
    /// Returns the verity device with the given ID, if it exists.
    pub fn verity_device(&self, device_id: &BlockDeviceId) -> Option<&VerityDevice> {
        self.verity.iter().find(|v| &v.id == device_id)
    }

    /// Returns a reference to the partition with the given ID, if it exists.
    ///
    /// This function searches through all disks and their partitions to find
    /// the partition with the given ID.
    pub fn get_partition(&self, id: &BlockDeviceId) -> Option<&Partition> {
        self.disks
            .iter()
            .flat_map(|d| d.partitions.iter())
            .find(|p| &p.id == id)
    }

    /// Builds a storage graph from the storage configuration.
    pub fn build_graph(&self) -> Result<StorageGraph, StorageGraphBuildError> {
        let mut builder = StorageGraphBuilder::default();

        // Add disks
        for disk in &self.disks {
            builder.add_node(disk.into());

            // Add partitions
            for partition in &disk.partitions {
                builder.add_node(partition.into());
            }

            // Add adopted partitions
            for adopted_partition in &disk.adopted_partitions {
                builder.add_node(adopted_partition.into());
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

        for fs in &self.filesystems {
            builder.add_node(fs.into());
        }

        for swap in &self.swap {
            builder.add_node(swap.into());
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
    ) -> Result<StorageGraph, HostConfigurationStaticValidationError> {
        // Check basic constraints

        if let Some(encryption) = &self.encryption {
            encryption.validate()?;
        }

        // Build the storage graph
        let graph = self.build_graph()?;

        // If storage configuration is requested, then
        if *self != Storage::default() {
            // ESP volume must be present, to update Grub configuration
            validate_volume_presence(&graph, ESP_MOUNT_POINT_PATH)?;
            // /var/tmp must not be on a read-only volume
            self.validate_writable_mount_points()?;
        }

        // Ensure the root mount point is present when:
        //  - Storage configuration is requested
        //  - Other subsystems require root mount point
        //  - Verity filesystems are present
        if require_root_mount_point || *self != Storage::default() || !self.verity.is_empty() {
            validate_volume_presence(&graph, ROOT_MOUNT_POINT_PATH)?;
        }

        // Validation of verity devices
        self.validate_verity_devices(&graph)?;

        Ok(graph)
    }

    /// Checks that mountpoints that are expected to be writable are mounted as
    /// writable. Currently only check /var/tmp.
    fn validate_writable_mount_points(&self) -> Result<(), HostConfigurationStaticValidationError> {
        // Ensure that /var/tmp is not on a read-only volume
        let var_tmp_mount_point = self.path_to_mount_point_info(VAR_TMP_PATH).ok_or(
            HostConfigurationStaticValidationError::ExpectedMountPointNotFound {
                mount_point_path: VAR_TMP_PATH.into(),
            },
        )?;
        if var_tmp_mount_point
            .mount_point
            .options
            .contains(MOUNT_OPTION_READ_ONLY)
        {
            return Err(
                HostConfigurationStaticValidationError::VarTmpOnReadOnlyVolume {
                    mount_point_path: var_tmp_mount_point
                        .mount_point
                        .path
                        .to_string_lossy()
                        .to_string(),
                },
            );
        }

        Ok(())
    }

    /// Validates the verity device configuration.
    fn validate_verity_devices(
        &self,
        graph: &StorageGraph,
    ) -> Result<(), HostConfigurationStaticValidationError> {
        // Return early if no verity devices are present
        if self.verity.is_empty() {
            trace!("No verity devices found in the host configuration, skipping validation");
            return Ok(());
        }

        trace!("Validating verity devices in the host configuration");

        // Trident supports at most one verity device. Verify that no more than one device is
        // listed.
        if self.verity.len() > 1 {
            return Err(HostConfigurationStaticValidationError::UnsupportedVerityDevices);
        }

        // Get the root verity device.
        let verity_device = &self.verity[0];

        // Get the filesystem placed on that device. We expect exactly one.
        let fs_on_verity = graph
            .filesystem_on_device(&verity_device.id)
            .ok_or(HostConfigurationStaticValidationError::UnsupportedVerityDevices)?;

        // Ensure the filesystem is mounted.
        let Some(mount_point) = fs_on_verity.mount_point.as_ref() else {
            return Err(
                HostConfigurationStaticValidationError::VerityFilesystemWithoutMountPoint {
                    device_name: verity_device.id.clone(),
                },
            );
        };

        // Ensure the filesystem is mounted read-only.
        if !mount_point.options.contains(MOUNT_OPTION_READ_ONLY) {
            return Err(
                HostConfigurationStaticValidationError::VerityDeviceMountedReadWrite {
                    device_name: verity_device.name.clone(),
                    mount_point_path: mount_point.path.to_string_lossy().to_string(),
                },
            );
        }

        // Now check the verity type...
        if mount_point.path == Path::new(ROOT_MOUNT_POINT_PATH) {
            self.validate_root_verity(graph, verity_device)
        } else if mount_point.path == Path::new(USR_MOUNT_POINT_PATH) {
            self.validate_usr_verity(verity_device)
        } else {
            Err(HostConfigurationStaticValidationError::UnsupportedVerityDevices)
        }
    }

    fn validate_root_verity(
        &self,
        graph: &StorageGraph,
        verity_device: &VerityDevice,
    ) -> Result<(), HostConfigurationStaticValidationError> {
        // If root verity is requested, we also require dedicated /boot
        // partition, as we otherwise cannot modify grub configuration and
        // kernel command line.
        validate_volume_presence(graph, BOOT_MOUNT_POINT_PATH)?;

        // For root verity, we also require an overlay for /etc, so that we can
        // inject configuration generated by Trident. This overlay needs to be
        // stored on a separate partition, as the root partition is read-only.
        // For the initial release, we are not exposing configuration of this
        // overlay backing partition to user, but instead, we will expect
        // /var/lib/trident-overlay to be present and use it as the backing
        // partition for the overlay. /var/lib/trident-overlay needs to be
        // backed by an A/B update volume pair and not reside on a read-only
        // volume.
        let overlay_support_mount_point =
            self.path_to_mount_point_info(TRIDENT_OVERLAY_PATH).ok_or(
                HostConfigurationStaticValidationError::ExpectedMountPointNotFound {
                    mount_point_path: TRIDENT_OVERLAY_PATH.into(),
                },
            )?;

        // Make sure the overlay is backed by a block device
        let overlay_block_device_id = overlay_support_mount_point.device_id.ok_or(
            HostConfigurationStaticValidationError::MountPointNotBackedByBlockDevice {
                mount_point_path: TRIDENT_OVERLAY_PATH.into(),
            },
        )?;

        // If some ab_update is present, the overlay must be also on an A/B volume.
        if let Some(ab_update) = &self.ab_update {
            if !ab_update
                .volume_pairs
                .iter()
                .any(|p| p.id == *overlay_block_device_id)
            {
                return Err(
                    HostConfigurationStaticValidationError::MountPointNotBackedByAbUpdateVolumePair {
                        mount_point_path: TRIDENT_OVERLAY_PATH.into(),
                    },
                );
            }
        }

        // Ensure the overlay is not on a read-only volume
        if overlay_support_mount_point
            .mount_point
            .options
            .contains(MOUNT_OPTION_READ_ONLY)
        {
            return Err(
                HostConfigurationStaticValidationError::OverlayOnReadOnlyVolume {
                    overlay_path: TRIDENT_OVERLAY_PATH.into(),
                    mount_point_path: overlay_support_mount_point
                        .mount_point
                        .path
                        .to_string_lossy()
                        .to_string(),
                },
            );
        }

        // Ensure the overlay is not on a verity protected volume
        if self
            .verity
            .iter()
            .any(|v| v.data_device_id.as_str() == overlay_block_device_id)
        {
            return Err(
                HostConfigurationStaticValidationError::OverlayOnVerityProtectedVolume {
                    overlay_path: TRIDENT_OVERLAY_PATH.into(),
                    mount_point_path: overlay_support_mount_point
                        .mount_point
                        .path
                        .to_string_lossy()
                        .to_string(),
                },
            );
        }

        // Ensure the root verity fs name is set to 'root', as that is what the dracut verity
        // module expects.
        if verity_device.name != ROOT_VERITY_DEVICE_NAME {
            return Err(
                HostConfigurationStaticValidationError::VerityDeviceNameInvalid {
                    device_name: verity_device.name.clone(),
                    expected: ROOT_VERITY_DEVICE_NAME.into(),
                },
            );
        }

        Ok(())
    }

    fn validate_usr_verity(
        &self,
        verity_device: &VerityDevice,
    ) -> Result<(), HostConfigurationStaticValidationError> {
        // Ensure the usr verity fs name is set to 'usr', for consistency with
        // the root verity deviceROOT_VERITY_DEVICE_NAME
        if verity_device.name != USR_VERITY_DEVICE_NAME {
            return Err(
                HostConfigurationStaticValidationError::VerityDeviceNameInvalid {
                    device_name: verity_device.name.clone(),
                    expected: USR_VERITY_DEVICE_NAME.into(),
                },
            );
        }

        Ok(())
    }

    /// Get an iterator over all the mount points in the storage configuration.
    pub fn mount_point_info(&self) -> impl Iterator<Item = MountPointInfo<'_>> {
        self.filesystems.iter().filter_map(|fs| {
            fs.mount_point.as_ref().map(|mp| MountPointInfo {
                mount_point: mp,
                device_id: fs.device_id.as_ref(),
            })
        })
    }

    /// Get a MountPointInfo instance for the device corresponding to the given device_id.
    pub fn device_id_to_mount_point_info(
        &self,
        device_id: &BlockDeviceId,
    ) -> Option<MountPointInfo<'_>> {
        self.mount_point_info()
            .find(|mp| mp.device_id == Some(device_id))
    }

    /// Get a MountPointInfo instance for the mount point that is holding the
    /// given path.
    pub fn path_to_mount_point_info(&self, path: impl AsRef<Path>) -> Option<MountPointInfo<'_>> {
        self.mount_point_info()
            .filter(|mp| path.as_ref().starts_with(&mp.mount_point.path))
            .max_by_key(|mp| mp.mount_point.path.as_os_str().len())
    }

    /// Validates whether the block device with device_id is the mount point for the directory at
    /// path.
    pub fn is_mount_point_for_path(
        &self,
        device_id: &BlockDeviceId,
        path: impl AsRef<Path>,
    ) -> bool {
        self.path_to_mount_point_info(path)
            .and_then(|mpi| mpi.device_id)
            == Some(device_id)
    }

    /// Returns the mount point and relative path for a given path.
    ///
    /// The mount point is the closest parent directory of the path that is a
    /// mount point. The relative path is the path relative to the mount point.
    pub fn get_mount_point_info_and_relative_path<'a, 'b>(
        &'a self,
        path: &'b Path,
    ) -> Option<(MountPointInfo<'a>, &'b Path)> {
        self.path_to_mount_point_info(path).and_then(move |mpi| {
            let rel_path = path.strip_prefix(&mpi.mount_point.path).ok()?;
            Some((mpi, rel_path))
        })
    }

    /// Returns a list of block device IDs that correspond to the A/B volume pairs.
    pub fn get_ab_volume_pair_ids(&self) -> HashSet<&BlockDeviceId> {
        self.ab_update
            .as_ref()
            .map(|ab| ab.volume_pairs.iter().map(|p| &p.id).collect())
            .unwrap_or_default()
    }

    /// Returns the filesystem which contains the given path.
    pub fn path_to_filesystem(&self, path: impl AsRef<Path>) -> Option<&FileSystem> {
        self.filesystems
            .iter()
            .filter(|fs| {
                fs.mount_point_path()
                    .is_some_and(|mpp| path.as_ref().starts_with(mpp))
            })
            .max_by_key(|fs| {
                fs.mount_point
                    .as_ref()
                    .map_or(0, |mp| mp.path.components().count())
            })
    }

    /// Get a map of all the mount points keyed by the mount point path.
    pub fn mount_points_by_path(&self) -> BTreeMap<&Path, MountPointInfo<'_>> {
        self.mount_point_info()
            .map(|mp| (mp.mount_point.path.as_path(), mp))
            .collect()
    }

    /// Returns whether the given device ID is an adopted partition.
    pub fn is_adopted_partition(&self, device_id: &BlockDeviceId) -> bool {
        self.disks
            .iter()
            .any(|d| d.adopted_partitions.iter().any(|p| &p.id == device_id))
    }

    /// Returns a reference to the ESP's device ID and filesystem, when
    /// available.
    ///
    /// The ESP filesystem is defined as having a block device and being mounted
    /// at ESP_MOUNT_POINT_PATH.
    pub fn esp_filesystem(&self) -> Option<(&BlockDeviceId, &FileSystem)> {
        self.filesystems.iter().find_map(|fs| {
            if fs
                .mount_point
                .as_ref()
                .is_some_and(|mp| mp.path.as_path() == Path::new(ESP_MOUNT_POINT_PATH))
            {
                fs.device_id.as_ref().map(|id| (id, fs))
            } else {
                None
            }
        })
    }
}

/// Validate that a volume is present and backed by an image or an adopted
/// filesystem.
fn validate_volume_presence(
    graph: &StorageGraph,
    path: impl AsRef<Path>,
) -> Result<(), HostConfigurationStaticValidationError> {
    match graph.get_volume_status(path.as_ref()) {
        VolumeStatus::PresentAndBackedByImage | VolumeStatus::PresentAndBackedByAdoptedFs => Ok(()),
        VolumeStatus::PresentButNotBackedByImage => Err(
            HostConfigurationStaticValidationError::MountPointNotBackedByImage {
                mount_point_path: path.as_ref().to_string_lossy().to_string(),
            },
        ),
        VolumeStatus::NotPresent => Err(
            HostConfigurationStaticValidationError::ExpectedMountPointNotFound {
                mount_point_path: path.as_ref().to_string_lossy().to_string(),
            },
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, str::FromStr};

    use storage_graph::{
        node::NodeIdentifier,
        types::{BlkDevKind, BlkDevReferrerKind},
    };
    use url::Url;

    use sysdefs::tpm2::Pcr;

    use crate::{
        config::HostConfiguration,
        constants::{BOOT_MOUNT_POINT_PATH, ROOT_MOUNT_POINT_PATH},
    };

    use self::{
        abupdate::AbVolumePair,
        disks::PartitionTableType,
        encryption::EncryptedVolume,
        filesystem::{FileSystemSource, MountOptions, MountPoint},
        filesystem_types::NewFileSystemType,
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
                    device: ROOT_MOUNT_POINT_PATH.into(),
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
                        Partition {
                            id: "var".to_owned(),
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
                    device_id: "srv-enc".to_owned(),
                }],
                pcrs: vec![Pcr::Pcr7],
            }),
            raid: Raid {
                software: vec![SoftwareRaidArray {
                    id: "mnt".to_owned(),
                    name: "md-mnt".to_owned(),
                    level: RaidLevel::Raid1,
                    devices: vec!["mnt-raid-1".to_owned(), "mnt-raid-2".to_owned()],
                }],
                ..Default::default()
            },
            filesystems: vec![
                FileSystem {
                    device_id: Some("esp".to_owned()),
                    source: FileSystemSource::Image,
                    mount_point: Some(MountPoint {
                        path: PathBuf::from(ESP_MOUNT_POINT_PATH),
                        options: MountOptions::empty(),
                    }),
                },
                FileSystem {
                    device_id: Some("boot".into()),
                    source: FileSystemSource::Image,
                    mount_point: Some(MountPoint {
                        path: PathBuf::from(BOOT_MOUNT_POINT_PATH),
                        options: MountOptions::empty(),
                    }),
                },
                FileSystem {
                    device_id: Some("root".into()),
                    source: FileSystemSource::Image,
                    mount_point: Some(MountPoint {
                        path: PathBuf::from(ROOT_MOUNT_POINT_PATH),
                        options: MountOptions::empty(),
                    }),
                },
                FileSystem {
                    device_id: Some("srv".into()),
                    source: FileSystemSource::New(NewFileSystemType::Ext4),
                    mount_point: Some(MountPoint {
                        path: PathBuf::from("/srv"),
                        options: MountOptions::empty(),
                    }),
                },
                FileSystem {
                    device_id: Some("overlay".into()),
                    source: FileSystemSource::New(NewFileSystemType::Ext4),
                    mount_point: Some(MountPoint {
                        path: PathBuf::from(TRIDENT_OVERLAY_PATH),
                        options: MountOptions::empty(),
                    }),
                },
                FileSystem {
                    device_id: Some("mnt".into()),
                    source: FileSystemSource::New(NewFileSystemType::Ext4),
                    mount_point: Some(MountPoint {
                        path: PathBuf::from("/mnt"),
                        options: MountOptions::empty(),
                    }),
                },
                FileSystem {
                    device_id: Some("var".into()),
                    source: FileSystemSource::New(NewFileSystemType::Ext4),
                    mount_point: Some(MountPoint {
                        path: PathBuf::from("/var"),
                        options: MountOptions::empty(),
                    }),
                },
            ],
            ab_update: Some(AbUpdate {
                volume_pairs: vec![AbVolumePair {
                    id: "root".to_owned(),
                    volume_a_id: "root-a".to_owned(),
                    volume_b_id: "root-b".to_owned(),
                }],
            }),
            ..Default::default()
        }
    }

    fn get_verity_storage() -> Storage {
        let mut storage = get_storage();

        // Delete the root fs, remove the A/B update (inactive) volume and replace it with a verity
        // filesystem.
        storage
            .filesystems
            .retain(|fs| fs.device_id != Some("root".into()));
        storage.ab_update = None;
        storage.verity = vec![VerityDevice {
            id: "root".into(),
            name: "root".into(),
            data_device_id: "root-a".into(),
            hash_device_id: "root-a-verity".into(),
            ..Default::default()
        }];
        storage.filesystems.push(FileSystem {
            device_id: Some("root".into()),
            source: FileSystemSource::Image,
            mount_point: Some(MountPoint {
                path: PathBuf::from(ROOT_MOUNT_POINT_PATH),
                options: MountOptions::new(MOUNT_OPTION_READ_ONLY),
            }),
        });

        storage
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
        assert_eq!(
            partition.size,
            crate::config::PartitionSize::Fixed(1048576.into())
        );

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
            filesystems: vec![
                FileSystem {
                    device_id: Some("disk1-partition1".to_string()),
                    source: FileSystemSource::Image,
                    mount_point: Some(MountPoint {
                        path: PathBuf::from(ESP_MOUNT_POINT_PATH),
                        options: MountOptions::empty(),
                    }),
                },
                FileSystem {
                    device_id: Some("disk1-partition2".to_string()),
                    source: FileSystemSource::Image,
                    mount_point: Some(MountPoint {
                        path: PathBuf::from(ROOT_MOUNT_POINT_PATH),
                        options: MountOptions::empty(),
                    }),
                },
            ],
            ..Default::default()
        };
        storage.validate(true).unwrap();

        let mount_volume_pair = Storage {
            ab_update: Some(AbUpdate {
                volume_pairs: vec![abupdate::AbVolumePair {
                    id: "ab-update-volume-pair".to_string(),
                    volume_a_id: "disk1-partition2".to_string(),
                    volume_b_id: "disk2-partition2".to_string(),
                }],
            }),
            filesystems: vec![
                FileSystem {
                    device_id: Some("ab-update-volume-pair".to_string()),
                    source: FileSystemSource::Image,
                    mount_point: Some(MountPoint {
                        path: PathBuf::from(ROOT_MOUNT_POINT_PATH),
                        options: MountOptions::empty(),
                    }),
                },
                FileSystem {
                    device_id: Some("disk1-partition1".to_string()),
                    source: FileSystemSource::Image,
                    mount_point: Some(MountPoint {
                        path: PathBuf::from(ESP_MOUNT_POINT_PATH),
                        options: MountOptions::empty(),
                    }),
                },
            ],
            ..storage.clone()
        };
        mount_volume_pair.validate(true).unwrap();

        let bad_volume_pair = Storage {
            ab_update: Some(AbUpdate {
                volume_pairs: vec![abupdate::AbVolumePair {
                    id: "ab-update-volume-pair".to_string(),
                    volume_a_id: "disk1-partition1".to_string(),
                    volume_b_id: "disk1-partition1".to_string(),
                }],
            }),
            ..storage.clone()
        };
        assert_eq!(
            bad_volume_pair.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidStorageGraph(
                StorageGraphBuildError::DuplicateTargetId {
                    node_identifier: NodeIdentifier::block_device("ab-update-volume-pair"),
                    kind: BlkDevReferrerKind::ABVolume,
                    target_id: "disk1-partition1".into(),
                }
            )
        );

        let bad_volume_pair_id = Storage {
            ab_update: Some(AbUpdate {
                volume_pairs: vec![abupdate::AbVolumePair {
                    id: "disk1".to_string(),
                    volume_a_id: "disk1-partition2".to_string(),
                    volume_b_id: "disk2-partition2".to_string(),
                }],
            }),
            ..storage.clone()
        };
        assert_eq!(
            bad_volume_pair_id.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidStorageGraph(
                StorageGraphBuildError::DuplicateDeviceId("disk1".into())
            )
        );

        let bad_filesystem_target = Storage {
            filesystems: vec![FileSystem {
                device_id: Some("disk99".to_string()),
                source: FileSystemSource::Image,
                mount_point: Some(MountPoint {
                    path: PathBuf::from("/some/path"),
                    options: MountOptions::empty(),
                }),
            }],
            ..storage.clone()
        };
        assert_eq!(
            bad_filesystem_target.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidStorageGraph(
                StorageGraphBuildError::NonExistentReference {
                    node_identifier: NodeIdentifier::from(&bad_filesystem_target.filesystems[0]),
                    kind: BlkDevReferrerKind::FileSystemImage,
                    target_id: "disk99".into(),
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
                    devices: vec!["part3".to_owned(), "part4".to_owned()],
                }],
                ..Default::default()
            },
            filesystems: vec![
                FileSystem {
                    device_id: Some("part1".to_owned()),
                    source: FileSystemSource::Image,
                    mount_point: Some(MountPoint {
                        path: PathBuf::from(ESP_MOUNT_POINT_PATH),
                        options: MountOptions::empty(),
                    }),
                },
                FileSystem {
                    device_id: Some("ab1".to_owned()),
                    source: FileSystemSource::Image,
                    mount_point: Some(MountPoint {
                        path: PathBuf::from(ROOT_MOUNT_POINT_PATH),
                        options: MountOptions::empty(),
                    }),
                },
            ],
            ab_update: Some(AbUpdate {
                volume_pairs: vec![AbVolumePair {
                    id: "ab1".to_owned(),
                    volume_a_id: "part5".to_owned(),
                    volume_b_id: "part2".to_owned(),
                }],
            }),
            ..Default::default()
        };
        storage.validate(true).unwrap();

        let storage_golden = storage.clone();

        // Fail on duplicate id.
        storage = storage_golden.clone();
        storage.disks.get_mut(0).unwrap().partitions = vec![Partition {
            id: "part1".to_owned(),
            partition_type: PartitionType::Esp,
            size: PartitionSize::from_str("1M").unwrap(),
        }];
        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidStorageGraph(
                StorageGraphBuildError::DuplicateDeviceId("part1".into())
            ),
        );

        // Fail on duplicate id.
        storage = storage_golden.clone();
        storage.ab_update.as_mut().unwrap().volume_pairs[0].id = "disk1".to_owned();
        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidStorageGraph(
                StorageGraphBuildError::DuplicateDeviceId("disk1".into())
            ),
        );

        // Fail on missing reference (disk4 does not exist).
        storage = storage_golden.clone();
        storage.ab_update.as_mut().unwrap().volume_pairs[0].volume_a_id = "disk4".to_owned();
        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidStorageGraph(
                StorageGraphBuildError::NonExistentReference {
                    node_identifier: NodeIdentifier::block_device("ab1"),
                    kind: BlkDevReferrerKind::ABVolume,
                    target_id: "disk4".into()
                }
            ),
        );

        // Fail on missing reference (disk4 does not exist).
        storage = storage_golden.clone();
        storage.filesystems[0].device_id = Some("disk4".into());
        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidStorageGraph(
                StorageGraphBuildError::NonExistentReference {
                    node_identifier: NodeIdentifier::from(&storage.filesystems[0]),
                    kind: BlkDevReferrerKind::FileSystemEsp,
                    target_id: "disk4".into(),
                }
            ),
        );

        // Fail on bad block device type.
        storage = storage_golden.clone();
        storage.filesystems[0].device_id = Some("disk1".into());
        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidStorageGraph(
                StorageGraphBuildError::InvalidReferenceKind {
                    node_identifier: NodeIdentifier::from(&storage.filesystems[0]),
                    kind: BlkDevReferrerKind::FileSystemEsp,
                    target_id: "disk1".into(),
                    target_kind: BlkDevKind::Disk,
                    valid_references: BlkDevReferrerKind::FileSystemEsp.compatible_kinds()
                }
            ),
        );

        // Fail if devices are not all the same size for a RAID.
        storage = storage_golden.clone();
        storage.disks[1].partitions[3].size = PartitionSize::from_str("2G").unwrap();
        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidStorageGraph(
                StorageGraphBuildError::PartitionSizeMismatch {
                    node_identifier: NodeIdentifier::block_device("my-raid1"),
                    kind: BlkDevReferrerKind::RaidArray
                }
            ),
        );
    }

    #[test]
    fn test_device_paths_absolute() {
        let mut storage = get_storage();
        storage.disks[0].device = "/dev/sda".into();
        // make sure it is ok
        storage.validate(true).unwrap();
    }

    #[test]
    fn test_device_paths_not_absolute() {
        let mut storage = get_storage();
        storage.disks[0].device = "disk1".into();
        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidStorageGraph(
                StorageGraphBuildError::BasicCheckFailed {
                    node_id: "disk1".into(),
                    kind: BlkDevKind::Disk,
                    body: "Path 'disk1' must be absolute".into()
                }
            )
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

        // Push a new partition to encrypt
        storage.disks[0].partitions.push(Partition {
            id: "srv-b-enc".to_owned(),
            partition_type: PartitionType::LinuxGeneric,
            size: PartitionSize::from_str("1G").unwrap(),
        });

        // Encrypt new partition
        storage
            .encryption
            .as_mut()
            .unwrap()
            .volumes
            .push(EncryptedVolume {
                id: "srv-b".to_owned(),
                device_name: "alt-b".to_owned(),
                device_id: "srv-b-enc".to_owned(),
            });

        // Delete mount point associated with "srv", otherwise this would fail
        storage
            .filesystems
            .retain(|mp| mp.device_id != Some("srv".into()));

        // Add a new A/B update volume pair for the alt volumes
        storage
            .ab_update
            .as_mut()
            .unwrap()
            .volume_pairs
            .push(AbVolumePair {
                id: "srv-ab".to_owned(),
                volume_a_id: "srv".to_owned(),
                volume_b_id: "srv-b".to_owned(),
            });

        storage.validate(true).unwrap();
    }

    /// A/B update volume pairs can target encrypted volumes (B)
    #[test]
    fn test_validate_ab_update_volume_pair_b_id_encryption_pass() {
        let mut storage: Storage = get_storage();
        // Add new test partitions
        storage.disks[0].partitions.push(Partition {
            id: "alt-a-enc".to_owned(),
            partition_type: PartitionType::LinuxGeneric,
            size: PartitionSize::from_str("1G").unwrap(),
        });
        storage.disks[0].partitions.push(Partition {
            id: "alt-b-enc".to_owned(),
            partition_type: PartitionType::LinuxGeneric,
            size: PartitionSize::from_str("1G").unwrap(),
        });
        // Encrypt alt a and alt b
        storage
            .encryption
            .as_mut()
            .unwrap()
            .volumes
            .push(EncryptedVolume {
                id: "alt-a".to_owned(),
                device_name: "alt-a".to_owned(),
                device_id: "alt-a-enc".to_owned(),
            });
        storage
            .encryption
            .as_mut()
            .unwrap()
            .volumes
            .push(EncryptedVolume {
                id: "alt-b".to_owned(),
                device_name: "alt-b".to_owned(),
                device_id: "alt-b-enc".to_owned(),
            });

        // Add a new A/B update volume pair for the alt volumes
        storage
            .ab_update
            .as_mut()
            .unwrap()
            .volume_pairs
            .push(AbVolumePair {
                id: "alt".to_owned(),
                volume_a_id: "alt-a".to_owned(),
                volume_b_id: "alt-b".to_owned(),
            });

        storage.validate(true).unwrap();
    }

    /// Software RAID arrays must have one or more devices
    #[test]
    fn test_validate_software_raid_array_no_devices_fail() {
        let mut storage: Storage = get_storage();
        storage.raid.software[0].devices = Vec::new();
        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidStorageGraph(
                StorageGraphBuildError::InvalidTargetCount {
                    node_identifier: NodeIdentifier::block_device("mnt"),
                    kind: BlkDevReferrerKind::RaidArray,
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
            HostConfigurationStaticValidationError::InvalidStorageGraph(
                StorageGraphBuildError::InvalidReferenceKind {
                    node_identifier: NodeIdentifier::block_device("mnt"),
                    kind: BlkDevReferrerKind::RaidArray,
                    target_id: "srv".into(),
                    target_kind: BlkDevKind::EncryptedVolume,
                    valid_references: BlkDevReferrerKind::RaidArray.compatible_kinds()
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
            HostConfigurationStaticValidationError::InvalidStorageGraph(
                StorageGraphBuildError::DuplicateDeviceId("disk1".into())
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
            HostConfigurationStaticValidationError::InvalidStorageGraph(
                StorageGraphBuildError::DuplicateDeviceId("esp".into())
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
            HostConfigurationStaticValidationError::InvalidStorageGraph(
                StorageGraphBuildError::DuplicateDeviceId("mnt".into())
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
            HostConfigurationStaticValidationError::InvalidStorageGraph(
                StorageGraphBuildError::DuplicateDeviceId("root".into())
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
                device_id: "alt-enc".to_owned(),
            });

        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidStorageGraph(
                StorageGraphBuildError::DuplicateDeviceId("srv".into())
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
                device_id: "alt-enc".to_owned(),
            });
        storage.filesystems.push(FileSystem {
            device_id: Some("alt".to_owned()),
            source: FileSystemSource::New(NewFileSystemType::Ext4),
            mount_point: Some(MountPoint {
                path: PathBuf::from("/alt"),
                options: MountOptions::empty(),
            }),
        });
        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidStorageGraph(
                StorageGraphBuildError::UniqueFieldConstraintError {
                    node_id: "alt".into(),
                    other_id: "srv".into(),
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
            HostConfigurationStaticValidationError::InvalidEncryptionRecoveryKeyUrlScheme {
                url: "https://www.example.com/recovery.key".into(),
                scheme: "https".into(),
            }
        );
    }

    /// Encrypted volume target ID must not be a home partition
    #[test]
    fn test_validate_encryption_target_id_home_fail() {
        let mut storage: Storage = get_storage();
        storage.disks[1]
            .partitions
            .iter_mut()
            .find(|p| p.id == "srv-enc")
            .unwrap()
            .partition_type = PartitionType::Home;
        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidStorageGraph(
                StorageGraphBuildError::InvalidPartitionType {
                    node_identifier: NodeIdentifier::block_device("srv"),
                    kind: BlkDevReferrerKind::EncryptedVolume,
                    partition_id: "srv-enc".into(),
                    partition_type: PartitionType::Home,
                    valid_types: BlkDevReferrerKind::EncryptedVolume.allowed_partition_types()
                }
            ),
        )
    }

    /// Encrypted volume device ID must not be an ESP partition
    #[test]
    fn test_validate_encryption_target_id_esp_fail() {
        let mut storage: Storage = get_storage();
        // Remove the filesystem associated with ESP
        storage
            .filesystems
            .retain(|fs| fs.device_id != Some("esp".into()));

        // Update the device ID of the encrypted volume
        storage.encryption.as_mut().unwrap().volumes[0].device_id = "esp".to_owned();

        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidStorageGraph(
                StorageGraphBuildError::InvalidPartitionType {
                    node_identifier: NodeIdentifier::block_device("srv"),
                    kind: BlkDevReferrerKind::EncryptedVolume,
                    partition_id: "esp".into(),
                    partition_type: PartitionType::Esp,
                    valid_types: BlkDevReferrerKind::EncryptedVolume.allowed_partition_types(),
                }
            ),
        );
    }

    /// Encrypted volume target ID must not be a root partition
    #[test]
    fn test_validate_encryption_target_id_root_fail() {
        let mut storage: Storage = get_storage();

        // add an alt root partition
        storage.disks[0].partitions.push(Partition {
            id: "alt-root".to_owned(),
            partition_type: PartitionType::Root,
            size: PartitionSize::from_str("1G").unwrap(),
        });

        // Encrypt alt root
        storage
            .encryption
            .as_mut()
            .unwrap()
            .volumes
            .push(EncryptedVolume {
                id: "alt".to_owned(),
                device_name: "luks-alt".to_owned(),
                device_id: "alt-root".to_owned(),
            });

        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidStorageGraph(
                StorageGraphBuildError::InvalidPartitionType {
                    node_identifier: NodeIdentifier::block_device("alt"),
                    kind: BlkDevReferrerKind::EncryptedVolume,
                    partition_id: "alt-root".into(),
                    partition_type: PartitionType::Root,
                    valid_types: BlkDevReferrerKind::EncryptedVolume.allowed_partition_types()
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
            .device_id = "root-b-verity".to_owned();
        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidStorageGraph(
                StorageGraphBuildError::InvalidPartitionType {
                    node_identifier: NodeIdentifier::block_device("srv"),
                    kind: BlkDevReferrerKind::EncryptedVolume,
                    partition_id: "root-b-verity".into(),
                    partition_type: PartitionType::RootVerity,
                    valid_types: BlkDevReferrerKind::EncryptedVolume.allowed_partition_types()
                }
            ),
            "Block device 'srv' of kind 'encrypted volume' references invalid targets"
        );
    }

    /// Encrypted volume target ID must not be a software RAID array of home partitions
    #[test]
    fn test_validate_encryption_target_id_raid_home_fail() {
        let mut storage: Storage = get_storage();

        // Delete the filesystem associated with mnt
        storage
            .filesystems
            .retain(|mp| mp.device_id != Some("mnt".into()));

        // Switch the encryption target to the mnt RAID array
        storage.encryption.as_mut().unwrap().volumes[0].device_id = "mnt".to_owned();

        // Change the partition type of the mnt-raid-1/2 partitions to home
        storage.disks[1]
            .partitions
            .iter_mut()
            .filter(|p| p.id.starts_with("mnt-raid"))
            .for_each(|p| {
                p.partition_type = PartitionType::Home;
            });

        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidStorageGraph(
                StorageGraphBuildError::InvalidPartitionType {
                    node_identifier: NodeIdentifier::block_device("srv"),
                    kind: BlkDevReferrerKind::EncryptedVolume,
                    partition_id: "mnt-raid-2".into(),
                    partition_type: PartitionType::Home,
                    valid_types: BlkDevReferrerKind::EncryptedVolume.allowed_partition_types()
                }
            ),
        );
    }

    /// Encrypted volume target ID must not be a software RAID array of esp partitions
    #[test]
    fn test_validate_encryption_target_id_raid_esp_fail() {
        let mut storage: Storage = get_storage();

        // Delete the filesystem associated with mnt
        storage
            .filesystems
            .retain(|mp| mp.device_id != Some("mnt".into()));

        // Switch the encryption target to the mnt RAID array
        storage.encryption.as_mut().unwrap().volumes[0].device_id = "mnt".to_owned();

        // Change the partition type of the mnt-raid-1/2 partitions to root
        storage.disks[1]
            .partitions
            .iter_mut()
            .filter(|p| p.id.starts_with("mnt-raid"))
            .for_each(|p| {
                p.partition_type = PartitionType::Esp;
            });

        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidStorageGraph(
                StorageGraphBuildError::InvalidPartitionType {
                    node_identifier: NodeIdentifier::block_device("srv"),
                    kind: BlkDevReferrerKind::EncryptedVolume,
                    partition_id: "mnt-raid-2".into(),
                    partition_type: PartitionType::Esp,
                    valid_types: BlkDevReferrerKind::EncryptedVolume.allowed_partition_types()
                }
            ),
        );
    }

    /// Encrypted volume target ID must not be a software RAID array of root partitions
    #[test]
    fn test_validate_encryption_target_id_raid_root_fail() {
        let mut storage: Storage = get_storage();

        // Delete the filesystem associated with mnt
        storage
            .filesystems
            .retain(|mp| mp.device_id != Some("mnt".into()));

        // Switch the encryption target to the mnt RAID array
        storage.encryption.as_mut().unwrap().volumes[0].device_id = "mnt".to_owned();

        // Change the partition type of the mnt-raid-1/2 partitions to root
        storage.disks[1]
            .partitions
            .iter_mut()
            .filter(|p| p.id.starts_with("mnt-raid"))
            .for_each(|p| {
                p.partition_type = PartitionType::Root;
            });

        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidStorageGraph(
                StorageGraphBuildError::InvalidPartitionType {
                    node_identifier: NodeIdentifier::block_device("srv"),
                    kind: BlkDevReferrerKind::EncryptedVolume,
                    partition_id: "mnt-raid-2".into(),
                    partition_type: PartitionType::Root,
                    valid_types: BlkDevReferrerKind::EncryptedVolume.allowed_partition_types()
                }
            ),
        );
    }

    /// Encrypted volume target ID must not be a software RAID array of root-verity partitions
    #[test]
    fn test_validate_encryption_target_id_raid_root_verity_fail() {
        let mut storage: Storage = get_storage();

        // Delete the filesystem associated with mnt
        storage
            .filesystems
            .retain(|mp| mp.device_id != Some("mnt".into()));

        // Switch the encryption target to the mnt RAID array
        storage.encryption.as_mut().unwrap().volumes[0].device_id = "mnt".to_owned();

        // Change the partition type of the mnt-raid-1/2 partitions to root
        storage.disks[1]
            .partitions
            .iter_mut()
            .filter(|p| p.id.starts_with("mnt-raid"))
            .for_each(|p| {
                p.partition_type = PartitionType::RootVerity;
            });

        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidStorageGraph(
                StorageGraphBuildError::InvalidPartitionType {
                    node_identifier: NodeIdentifier::block_device("srv"),
                    kind: BlkDevReferrerKind::EncryptedVolume,
                    partition_id: "mnt-raid-2".into(),
                    partition_type: PartitionType::RootVerity,
                    valid_types: BlkDevReferrerKind::EncryptedVolume.allowed_partition_types()
                }
            ),
        );
    }

    /// Encrypted volume target ID must not be a software RAID array of no devices.
    #[test]
    fn test_validate_encryption_target_id_raid_no_devices_fail() {
        let mut storage: Storage = get_storage();
        storage.encryption.as_mut().unwrap().volumes[0].device_id = "mnt".to_owned();
        storage.raid.software[0].devices = Vec::new();
        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidStorageGraph(
                StorageGraphBuildError::InvalidTargetCount {
                    node_identifier: NodeIdentifier::block_device("mnt"),
                    kind: BlkDevReferrerKind::RaidArray,
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
        storage.encryption.as_mut().unwrap().volumes[0].device_id = "mnt".to_owned();
        storage.raid.software[0].devices = vec!["root".to_owned()];
        // Remove the first mount point
        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidStorageGraph(
                StorageGraphBuildError::InvalidTargetCount {
                    node_identifier: NodeIdentifier::block_device("mnt"),
                    kind: BlkDevReferrerKind::RaidArray,
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
        storage.encryption.as_mut().unwrap().volumes[0].device_id = "disk1".to_owned();
        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidStorageGraph(
                StorageGraphBuildError::InvalidReferenceKind {
                    node_identifier: NodeIdentifier::block_device("srv"),
                    kind: BlkDevReferrerKind::EncryptedVolume,
                    target_id: "disk1".into(),
                    target_kind: BlkDevKind::Disk,
                    valid_references: BlkDevReferrerKind::EncryptedVolume.compatible_kinds()
                }
            )
        );
    }

    /// Encrypted volume target ID can be a software RAID array instead of a partition
    #[test]
    fn test_validate_encryption_target_id_raid_pass() {
        let mut storage: Storage = get_storage();
        // Remove the mount point associated with "mnt"
        storage
            .filesystems
            .retain(|fs| fs.device_id != Some("mnt".into()));

        // Change the target ID of the encrypted volume to the RAID array
        storage.encryption.as_mut().unwrap().volumes[0].device_id = "mnt".to_owned();

        storage.validate(true).unwrap();
    }

    /// Encrypted volume target ID must not be an A/B update volume pair
    #[test]
    fn test_validate_encryption_target_id_ab_update_volume_pair_fail() {
        let mut storage: Storage = get_storage();

        // Remove filesystem associated with "root"
        storage
            .filesystems
            .retain(|fs| fs.device_id != Some("root".into()));

        // Change the target ID of the encrypted volume to the A/B update volume pair
        storage.encryption.as_mut().unwrap().volumes[0].device_id = "root".to_owned();

        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidStorageGraph(
                StorageGraphBuildError::InvalidReferenceKind {
                    node_identifier: NodeIdentifier::block_device("srv"),
                    kind: BlkDevReferrerKind::EncryptedVolume,
                    target_id: "root".into(),
                    target_kind: BlkDevKind::ABVolume,
                    valid_references: BlkDevReferrerKind::EncryptedVolume.compatible_kinds()
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
                device_id: "srv-enc".to_owned(),
            });
        storage.filesystems.push(FileSystem {
            device_id: Some("alt".to_owned()),
            source: FileSystemSource::New(NewFileSystemType::Ext4),
            mount_point: Some(MountPoint {
                path: PathBuf::from("/alt"),
                options: MountOptions::empty(),
            }),
        });
        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidStorageGraph(
                StorageGraphBuildError::ReferrerForbiddenSharing {
                    target_id: "srv-enc".into(),
                    target_kind: BlkDevKind::Partition,
                    referrer_a_id: NodeIdentifier::block_device("alt"),
                    referrer_a_kind: BlkDevReferrerKind::EncryptedVolume,
                    referrer_a_valid_sharing_peers: BlkDevReferrerKind::EncryptedVolume
                        .valid_sharing_peers(),
                    referrer_b_id: NodeIdentifier::block_device("srv"),
                    referrer_b_kind: BlkDevReferrerKind::EncryptedVolume,
                    referrer_b_valid_sharing_peers: BlkDevReferrerKind::EncryptedVolume
                        .valid_sharing_peers(),
                }
            )
        );
    }

    /// Encrypted volumes cannot target the same partition as a filesystem/mount point
    #[test]
    fn test_validate_encryption_mount_point_target_part_id_equal_fail() {
        let mut storage: Storage = get_storage();

        // Add a new filesystem to the partition used for encryption
        storage.filesystems.push(FileSystem {
            device_id: Some("srv-enc".to_owned()),
            source: FileSystemSource::New(NewFileSystemType::Ext4),
            mount_point: Some(MountPoint {
                path: PathBuf::from("/mnt/some-mount-point"),
                options: MountOptions::empty(),
            }),
        });

        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidStorageGraph(
                StorageGraphBuildError::ReferrerForbiddenSharing {
                    target_id: "srv-enc".into(),
                    target_kind: BlkDevKind::Partition,
                    referrer_a_id: NodeIdentifier::from(storage.filesystems.last().unwrap()),
                    referrer_a_kind: BlkDevReferrerKind::FileSystemNew,
                    referrer_a_valid_sharing_peers: BlkDevReferrerKind::FileSystemNew
                        .valid_sharing_peers(),
                    referrer_b_id: NodeIdentifier::block_device("srv"),
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
        storage.encryption.as_mut().unwrap().volumes[0].device_id = "mnt-raid-1".to_owned();
        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidStorageGraph(
                StorageGraphBuildError::ReferrerForbiddenSharing {
                    target_id: "mnt-raid-1".into(),
                    target_kind: BlkDevKind::Partition,
                    referrer_a_id: NodeIdentifier::block_device("srv"),
                    referrer_a_kind: BlkDevReferrerKind::EncryptedVolume,
                    referrer_a_valid_sharing_peers: BlkDevReferrerKind::EncryptedVolume
                        .valid_sharing_peers(),
                    referrer_b_id: NodeIdentifier::block_device("mnt"),
                    referrer_b_kind: BlkDevReferrerKind::RaidArray,
                    referrer_b_valid_sharing_peers: BlkDevReferrerKind::RaidArray
                        .valid_sharing_peers(),
                }
            ),
        );
    }

    /// Encrypted volumes cannot target the same partition as A/B update volume pair (A)
    #[test]
    fn test_validate_encryption_ab_update_volume_pair_a_part_id_equal_fail() {
        let mut storage: Storage = get_storage();
        storage.encryption.as_mut().unwrap().volumes[0].device_id = "root-a".to_owned();
        storage.disks[1].partitions[1].partition_type = PartitionType::LinuxGeneric;
        storage.disks[1].partitions[3].partition_type = PartitionType::LinuxGeneric;
        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidStorageGraph(
                StorageGraphBuildError::ReferrerForbiddenSharing {
                    target_id: "root-a".into(),
                    target_kind: BlkDevKind::Partition,
                    referrer_a_id: NodeIdentifier::block_device("srv"),
                    referrer_a_kind: BlkDevReferrerKind::EncryptedVolume,
                    referrer_a_valid_sharing_peers: BlkDevReferrerKind::EncryptedVolume
                        .valid_sharing_peers(),
                    referrer_b_id: NodeIdentifier::block_device("root"),
                    referrer_b_kind: BlkDevReferrerKind::ABVolume,
                    referrer_b_valid_sharing_peers: BlkDevReferrerKind::ABVolume
                        .valid_sharing_peers(),
                }
            ),
        );
    }

    /// Encrypted volumes cannot target the same partition as A/B update volume pair (B)
    #[test]
    fn test_validate_encryption_ab_update_volume_pair_b_part_id_equal_fail() {
        let mut storage: Storage = get_storage();
        storage.encryption.as_mut().unwrap().volumes[0].device_id = "root-b".to_owned();
        storage.disks[1].partitions[1].partition_type = PartitionType::LinuxGeneric;
        storage.disks[1].partitions[3].partition_type = PartitionType::LinuxGeneric;
        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidStorageGraph(
                StorageGraphBuildError::ReferrerForbiddenSharing {
                    target_id: "root-b".into(),
                    target_kind: BlkDevKind::Partition,
                    referrer_a_id: NodeIdentifier::block_device("srv"),
                    referrer_a_kind: BlkDevReferrerKind::EncryptedVolume,
                    referrer_a_valid_sharing_peers: BlkDevReferrerKind::EncryptedVolume
                        .valid_sharing_peers(),
                    referrer_b_id: NodeIdentifier::block_device("root"),
                    referrer_b_kind: BlkDevReferrerKind::ABVolume,
                    referrer_b_valid_sharing_peers: BlkDevReferrerKind::ABVolume
                        .valid_sharing_peers(),
                }
            ),
        );
    }

    /// Encrypted volumes cannot target the same software RAID array as an A/B update volume pair (A)
    #[test]
    fn test_validate_encryption_ab_update_volume_pair_a_raid_id_equal_fail() {
        let mut storage: Storage = get_storage();

        storage.disks[0].partitions.push(Partition {
            id: "alt-a-enc".to_owned(),
            partition_type: PartitionType::LinuxGeneric,
            size: PartitionSize::from_str("1G").unwrap(),
        });
        storage.disks[0].partitions.push(Partition {
            id: "alt-b-enc".to_owned(),
            partition_type: PartitionType::LinuxGeneric,
            size: PartitionSize::from_str("1G").unwrap(),
        });
        // Encrypt alt a and alt b
        storage
            .encryption
            .as_mut()
            .unwrap()
            .volumes
            .push(EncryptedVolume {
                id: "alt-a".to_owned(),
                device_name: "alt-a".to_owned(),
                device_id: "alt-a-enc".to_owned(),
            });

        // Add a new A/B update volume pair for the alt volumes
        storage
            .ab_update
            .as_mut()
            .unwrap()
            .volume_pairs
            .push(AbVolumePair {
                id: "alt".to_owned(),
                volume_a_id: "alt-a-enc".to_owned(),
                volume_b_id: "alt-b-enc".to_owned(),
            });

        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidStorageGraph(
                StorageGraphBuildError::ReferrerForbiddenSharing {
                    target_id: "alt-a-enc".into(),
                    target_kind: BlkDevKind::Partition,
                    referrer_a_id: NodeIdentifier::block_device("alt-a"),
                    referrer_a_kind: BlkDevReferrerKind::EncryptedVolume,
                    referrer_a_valid_sharing_peers: BlkDevReferrerKind::EncryptedVolume
                        .valid_sharing_peers(),
                    referrer_b_id: NodeIdentifier::block_device("alt"),
                    referrer_b_kind: BlkDevReferrerKind::ABVolume,
                    referrer_b_valid_sharing_peers: BlkDevReferrerKind::ABVolume
                        .valid_sharing_peers(),
                }
            ),
        );
    }

    /// Encrypted volumes cannot target the same software RAID array as an A/B update volume pair (B)
    #[test]
    fn test_validate_encryption_ab_update_volume_pair_b_raid_id_equal_fail() {
        let mut storage: Storage = get_storage();

        storage.disks[0].partitions.push(Partition {
            id: "alt-a-enc".to_owned(),
            partition_type: PartitionType::LinuxGeneric,
            size: PartitionSize::from_str("1G").unwrap(),
        });
        storage.disks[0].partitions.push(Partition {
            id: "alt-b-enc".to_owned(),
            partition_type: PartitionType::LinuxGeneric,
            size: PartitionSize::from_str("1G").unwrap(),
        });
        // Encrypt alt a and alt b
        storage
            .encryption
            .as_mut()
            .unwrap()
            .volumes
            .push(EncryptedVolume {
                id: "alt-b".to_owned(),
                device_name: "alt-b".to_owned(),
                device_id: "alt-b-enc".to_owned(),
            });

        // Add a new A/B update volume pair for the alt volumes
        storage
            .ab_update
            .as_mut()
            .unwrap()
            .volume_pairs
            .push(AbVolumePair {
                id: "alt".to_owned(),
                volume_a_id: "alt-a-enc".to_owned(),
                volume_b_id: "alt-b-enc".to_owned(),
            });

        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidStorageGraph(
                StorageGraphBuildError::ReferrerForbiddenSharing {
                    target_id: "alt-b-enc".into(),
                    target_kind: BlkDevKind::Partition,
                    referrer_a_id: NodeIdentifier::block_device("alt-b"),
                    referrer_a_kind: BlkDevReferrerKind::EncryptedVolume,
                    referrer_a_valid_sharing_peers: BlkDevReferrerKind::EncryptedVolume
                        .valid_sharing_peers(),
                    referrer_b_id: NodeIdentifier::block_device("alt"),
                    referrer_b_kind: BlkDevReferrerKind::ABVolume,
                    referrer_b_valid_sharing_peers: BlkDevReferrerKind::ABVolume
                        .valid_sharing_peers(),
                }
            ),
        );
    }

    /// Images can target encrypted volumes
    #[test]
    fn test_validate_image_target_id_encryption_pass() {
        let mut storage: Storage = get_storage();

        // Set the srv filesystem to use an image as source
        storage
            .filesystems
            .iter_mut()
            .find(|fs| fs.device_id == Some("srv".into()))
            .unwrap()
            .source = FileSystemSource::Image;

        storage.validate(true).unwrap();
    }

    #[test]
    fn test_validate_verity_pass() {
        get_verity_storage().validate(true).unwrap();
    }

    #[test]
    fn test_validate_verity_rw_fail() {
        let mut storage: Storage = get_verity_storage();

        // Remove "ro" from the mount options
        storage
            .filesystems
            .iter_mut()
            .filter_map(|fs| fs.mount_point.as_mut())
            .find(|mp| mp.path == Path::new(ROOT_MOUNT_POINT_PATH))
            .unwrap()
            .options = MountOptions::empty();

        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::VerityDeviceMountedReadWrite {
                mount_point_path: "/".into(),
                device_name: "root".into(),
            }
        );
    }

    #[test]
    fn test_validate_verity_bad_device_name_fail() {
        let mut storage: Storage = get_verity_storage();

        // Swap the name
        storage.verity[0].name = "verity-root-a".into();

        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::VerityDeviceNameInvalid {
                device_name: "verity-root-a".into(),
                expected: "root".into(),
            }
        );
    }

    #[test]
    fn test_validate_verity_without_boot_image_fail() {
        let mut storage: Storage = get_verity_storage();

        // Change the boot fs to create instead of image
        storage.filesystems[1].source = FileSystemSource::New(NewFileSystemType::default());

        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::MountPointNotBackedByImage {
                mount_point_path: "/boot".into()
            },
        );
    }

    #[test]
    fn test_validate_verity_without_boot_mountpoint_fail() {
        let mut storage: Storage = get_verity_storage();

        // Delete the boot fs
        storage
            .filesystems
            .retain(|fs| fs.device_id != Some("boot".into()));

        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::ExpectedMountPointNotFound {
                mount_point_path: "/boot".into()
            },
        );
    }

    #[test]
    fn test_validate_verity_ro_overlay_fail() {
        let mut storage: Storage = get_verity_storage();

        // Set the overlay fs to read-only
        storage
            .filesystems
            .iter_mut()
            .find(|fs| fs.device_id == Some("overlay".into()))
            .unwrap()
            .mount_point
            .as_mut()
            .unwrap()
            .options = MountOptions::new(MOUNT_OPTION_READ_ONLY);

        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::OverlayOnReadOnlyVolume {
                overlay_path: "/var/lib/trident-overlay".into(),
                mount_point_path: "/var/lib/trident-overlay".into(),
            }
        );
    }

    #[test]
    fn test_validate_writable_mount_points_pass() {
        let storage: Storage = get_verity_storage();
        storage.validate(true).unwrap();

        let mut storage: Storage = get_storage();
        storage.validate(true).unwrap();

        // Remove the var filesystem, should be ok, as / is rw
        storage
            .filesystems
            .retain(|fs| fs.device_id != Some("var".into()));

        storage.validate(true).unwrap();
    }

    #[test]
    fn test_validate_writable_mount_points_fails() {
        let mut storage: Storage = get_verity_storage();

        // Remove the var filesystem, which is rw
        storage
            .filesystems
            .retain(|fs| fs.device_id != Some("var".into()));

        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::VarTmpOnReadOnlyVolume {
                mount_point_path: "/".into(),
            }
        );
    }

    #[test]
    fn test_validate_host_configuration_esp_on_raid() {
        let mut storage = Storage {
            filesystems: vec![
                FileSystem {
                    source: FileSystemSource::Image,
                    device_id: Some("esp".to_string()),
                    mount_point: Some(MountPoint {
                        path: ESP_MOUNT_POINT_PATH.into(),
                        options: MountOptions::defaults(),
                    }),
                },
                FileSystem {
                    device_id: Some("root".into()),
                    source: FileSystemSource::Image,
                    mount_point: Some(MountPoint {
                        path: PathBuf::from(ROOT_MOUNT_POINT_PATH),
                        options: MountOptions::empty(),
                    }),
                },
                FileSystem {
                    device_id: Some("var".into()),
                    source: FileSystemSource::New(NewFileSystemType::Ext4),
                    mount_point: Some(MountPoint {
                        path: PathBuf::from("/var"),
                        options: MountOptions::empty(),
                    }),
                },
            ],
            disks: vec![Disk {
                id: "disk1".into(),
                device: "/dev/sdb".into(),
                partitions: vec![
                    Partition {
                        id: "esp1".into(),
                        size: PartitionSize::from_str("512M").unwrap(),
                        partition_type: PartitionType::Esp,
                    },
                    Partition {
                        id: "esp2".into(),
                        size: PartitionSize::from_str("512M").unwrap(),
                        partition_type: PartitionType::Esp,
                    },
                    Partition {
                        id: "var".to_owned(),
                        partition_type: PartitionType::LinuxGeneric,
                        size: PartitionSize::from_str("1G").unwrap(),
                    },
                    Partition {
                        id: "root".to_owned(),
                        partition_type: PartitionType::Root,
                        size: PartitionSize::from_str("1G").unwrap(),
                    },
                ],
                ..Default::default()
            }],
            raid: Raid {
                software: vec![SoftwareRaidArray {
                    id: "esp".into(),
                    name: "esp".to_string(),
                    level: RaidLevel::Raid1,
                    devices: vec!["esp1".into(), "esp2".into()],
                }],
                sync_timeout: Some(180),
            },
            ..Default::default()
        };

        storage.validate(true).unwrap();

        // Change the RAID level to RAID0
        storage.raid.software[0].level = RaidLevel::Raid0;

        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidStorageGraph(
                StorageGraphBuildError::InvalidRaidlevel {
                    node_identifier: NodeIdentifier::from(&storage.filesystems[0]),
                    kind: BlkDevReferrerKind::FileSystemEsp,
                    raid_id: "esp".into(),
                    raid_level: RaidLevel::Raid0,
                    valid_levels: BlkDevReferrerKind::FileSystemEsp
                        .allowed_raid_levels()
                        .unwrap(),
                }
            )
        );

        // Change the RAID level to RAID5
        storage.raid.software[0].level = RaidLevel::Raid5;
        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidStorageGraph(
                StorageGraphBuildError::InvalidRaidlevel {
                    node_identifier: NodeIdentifier::from(&storage.filesystems[0]),
                    kind: BlkDevReferrerKind::FileSystemEsp,
                    raid_id: "esp".into(),
                    raid_level: RaidLevel::Raid5,
                    valid_levels: BlkDevReferrerKind::FileSystemEsp
                        .allowed_raid_levels()
                        .unwrap(),
                }
            )
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
                        devices: vec!["part3".to_owned(), "part4".to_owned()],
                    }],
                    ..Default::default()
                },
                filesystems: vec![FileSystem {
                    device_id: Some("part1".to_owned()),
                    source: FileSystemSource::Image,
                    mount_point: Some(MountPoint {
                        path: PathBuf::from(ROOT_MOUNT_POINT_PATH),
                        options: MountOptions::empty(),
                    }),
                }],
                ab_update: Some(AbUpdate {
                    volume_pairs: vec![AbVolumePair {
                        id: "ab1".to_owned(),
                        volume_a_id: "part1".to_owned(),
                        volume_b_id: "part2".to_owned(),
                    }],
                }),
                ..Default::default()
            },
            ..Default::default()
        };

        let mount_point = host_config
            .storage
            .path_to_mount_point_info(Path::new(ROOT_MOUNT_POINT_PATH).join("boot"))
            .unwrap();
        assert_eq!(mount_point.device_id, Some(&"part1".into()));

        // ensure to pick the longest prefix
        host_config.storage.filesystems.push(FileSystem {
            device_id: Some("part2".to_owned()),
            source: FileSystemSource::New(NewFileSystemType::Ext4),
            mount_point: Some(MountPoint {
                path: PathBuf::from(ROOT_MOUNT_POINT_PATH).join("boot"),
                options: MountOptions::empty(),
            }),
        });

        let mount_point = host_config
            .storage
            .path_to_mount_point_info(Path::new(ROOT_MOUNT_POINT_PATH).join("boot"))
            .unwrap();
        assert_eq!(mount_point.device_id, Some(&"part2".into()));

        // validate longer paths
        let mount_point = host_config
            .storage
            .path_to_mount_point_info(Path::new(ROOT_MOUNT_POINT_PATH).join("boot/foo/bar"))
            .unwrap();
        assert_eq!(mount_point.device_id, Some(&"part2".into()));

        let mount_point = host_config
            .storage
            .path_to_mount_point_info(Path::new(ROOT_MOUNT_POINT_PATH).join("foo/bar"))
            .unwrap();
        assert_eq!(mount_point.device_id, Some(&"part1".into()));

        // validate failure without any mount points
        host_config.storage.filesystems.clear();
        assert!(host_config
            .storage
            .path_to_mount_point_info(Path::new(ROOT_MOUNT_POINT_PATH).join("boot"))
            .is_none());
    }

    /// Validates that is_mount_point_for_path() correctly determines whether the block device is
    /// a mount point for a specified path.
    #[test]
    fn test_is_mount_point_for_path() {
        // Set up a host configuration with a few filesystems
        let host_config = HostConfiguration {
            storage: Storage {
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
                            path: PathBuf::from("/esp"),
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

        // Test case 1: Validate that 'root' is a mount point for /
        assert!(
            host_config
                .storage
                .is_mount_point_for_path(&"root".to_string(), Path::new("/")),
            "Block device with device_id 'root' was not identified as mount point for /"
        );

        // Test case 2: Validate that 'root' is not a mount point for /esp
        assert!(
            !host_config
                .storage
                .is_mount_point_for_path(&"root".to_string(), Path::new("/esp")),
            "Block device with device_id 'root' was incorrectly identified as mount point for /esp"
        );

        // Test case 3: Validate that 'esp' is a mount point for /esp
        assert!(
            host_config
                .storage
                .is_mount_point_for_path(&"esp".to_string(), Path::new("/esp")),
            "Block device with device_id 'esp' was not identified as mount point for /esp"
        );

        // Test case 4: Validate that 'trident' is not a mount point for a non-existent path /trident
        assert!(
            !host_config
                .storage
                .is_mount_point_for_path(&"trident".to_string(), Path::new("/trident")),
            "Block device with device_id 'trident' was incorrectly identified as mount point for /trident"
        );
    }

    #[test]
    fn test_validate_usr_verity() {
        // Ok
        Storage::default()
            .validate_usr_verity(&VerityDevice {
                id: "usr".into(),
                name: "usr".into(),
                data_device_id: "some-data-device".into(),
                hash_device_id: "some-hash-device".into(),
                ..Default::default()
            })
            .expect("Failed to validate usr verity device");

        // Bad name
        assert_eq!(
            Storage::default()
                .validate_usr_verity(&VerityDevice {
                    id: "usr".into(),
                    name: "usr-foo".into(),
                    data_device_id: "some-data-device".into(),
                    hash_device_id: "some-hash-device".into(),
                    ..Default::default()
                })
                .unwrap_err(),
            HostConfigurationStaticValidationError::VerityDeviceNameInvalid {
                device_name: "usr-foo".into(),
                expected: "usr".into(),
            }
        );
    }
}
