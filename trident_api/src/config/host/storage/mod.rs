use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
};

use blkdev_graph::types::BlkDevNode;
use serde::{Deserialize, Serialize};

#[cfg(feature = "schemars")]
use schemars::JsonSchema;

use crate::{
    constants::{
        BOOT_MOUNT_POINT_PATH, ESP_MOUNT_POINT_PATH, ROOT_MOUNT_POINT_PATH, TRIDENT_OVERLAY_PATH,
    },
    is_default, BlockDeviceId,
};

use super::error::HostConfigurationStaticValidationError;

pub mod blkdev_graph;
pub mod disks;
pub mod encryption;
pub mod filesystem;
pub mod imaging;
pub mod internal;
pub mod partitions;
pub mod raid;
mod serde_hash;

use self::{
    blkdev_graph::{
        builder::BlockDeviceGraphBuilder,
        error::BlockDeviceGraphBuildError,
        graph::{BlockDeviceGraph, VolumeStatus},
    },
    disks::Disk,
    encryption::Encryption,
    filesystem::{FileSystem, MountPointInfo, VerityFileSystem},
    imaging::{AbUpdate, Image},
    internal::{InternalMountPoint, InternalVerityDevice},
    partitions::Partition,
    raid::Raid,
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

    /// A/B update configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ab_update: Option<AbUpdate>,

    /// Filesystems in this host.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filesystems: Vec<FileSystem>,

    /// Verity filesystems in this host.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub verity_filesystems: Vec<VerityFileSystem>,

    /// Old API for mount points.
    ///
    /// Used internally by Trident-Core.
    #[serde(skip)]
    pub internal_mount_points: Vec<InternalMountPoint>,

    /// Old API for verity devices.
    ///
    /// Used internally by Trident-Core.
    #[serde(skip)]
    pub internal_verity: Vec<InternalVerityDevice>,
}

impl Storage {
    pub fn get_partition(&self, id: &BlockDeviceId) -> Option<&Partition> {
        self.disks
            .iter()
            .flat_map(|d| d.partitions.iter())
            .find(|p| &p.id == id)
    }

    /// Verifies if the partition is a raw partition. A raw partition is one that
    /// does not contain a filesystem or verity-filesystem, and is not part of any
    /// RAID array or encryption volume.
    pub fn is_raw_partition(
        &self,
        nodes: &BTreeMap<String, BlkDevNode>,
        partition_id: &BlockDeviceId,
    ) -> bool {
        if let Some(node) = nodes.get(partition_id) {
            if node.dependents.is_empty() && node.targets.is_empty() && node.filesystem.is_none() {
                return true;
            }
        }

        false
    }

    pub fn build_graph(&self) -> Result<BlockDeviceGraph<'_>, BlockDeviceGraphBuildError> {
        let mut builder = BlockDeviceGraphBuilder::default();

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

        for fs in &self.filesystems {
            builder.add_filesystem(fs);
        }

        for vfs in &self.verity_filesystems {
            builder.add_verity_filesystem(vfs);
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
    ) -> Result<(), HostConfigurationStaticValidationError> {
        // Check basic constraints

        if let Some(encryption) = &self.encryption {
            encryption.validate()?;
        }

        // Build the graph
        let graph = self.build_graph()?;

        // If storage configuration is requested, then ESP volume must be
        // present, to update Grub configuration
        if *self != Storage::default() {
            Self::validate_volume_presence(&graph, ESP_MOUNT_POINT_PATH)?;
        }

        // Ensure the root mount point is present when:
        //  - Storage configuration is requested
        //  - Other modules require root mount point
        //  - Verity filesystems are present
        if require_root_mount_point
            || *self != Storage::default()
            || !self.verity_filesystems.is_empty()
        {
            Self::validate_volume_presence(&graph, ROOT_MOUNT_POINT_PATH)?;
        }

        // Validate verity configuration
        // Depends on root mount point validated above
        self.validate_verity(&graph)?;

        Ok(())
    }

    /// Validate that a volume is present and backed by an image or an adopted
    /// filesystem.
    fn validate_volume_presence(
        graph: &BlockDeviceGraph,
        path: impl AsRef<Path>,
    ) -> Result<(), HostConfigurationStaticValidationError> {
        match graph.get_volume_status(path.as_ref()) {
            VolumeStatus::PresentAndBackedByImage | VolumeStatus::PresentAndBackedByAdoptedFs => {
                Ok(())
            }
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

    /// Validates the verity configuration. Assumes the verity list of devices
    /// is not empty.
    fn validate_verity(
        &self,
        graph: &BlockDeviceGraph,
    ) -> Result<(), HostConfigurationStaticValidationError> {
        // Return early if no verity filesystems are present
        if self.verity_filesystems.is_empty() {
            return Ok(());
        }

        // Verity is only supported for root volume, verify the input is not
        // asking for something else
        if self.verity_filesystems.len() > 1 {
            return Err(HostConfigurationStaticValidationError::UnsupportedVerityDevices);
        }

        // Get the root verity fs
        let vfs = &self.verity_filesystems[0];

        // Ensure the verity fs is mounted at root
        if vfs.mount_point.path != Path::new(ROOT_MOUNT_POINT_PATH) {
            return Err(HostConfigurationStaticValidationError::UnsupportedVerityDevices);
        }

        // If root verity is required, we also require dedicated /boot
        // partition, as we otherwise cannot modify grub configuration and
        // kernel command line.
        Self::validate_volume_presence(graph, BOOT_MOUNT_POINT_PATH)?;

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
            .contains("ro")
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
            .verity_filesystems
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
        if vfs.name != "root" {
            return Err(
                HostConfigurationStaticValidationError::RootVerityDeviceNameInvalid {
                    device_name: vfs.name.clone(),
                },
            );
        }

        // Ensure the root verity device is mounted read-only at /.
        if !vfs.mount_point.options.contains("ro") {
            return Err(
                HostConfigurationStaticValidationError::VerityDeviceMountedReadWrite {
                    device_name: vfs.name.clone(),
                    mount_point_path: vfs.mount_point.path.to_string_lossy().to_string(),
                },
            );
        }

        Ok(())
    }

    /// Get an iterator over all the mount points in the storage configuration.
    pub fn mount_point_info(&self) -> impl Iterator<Item = MountPointInfo<'_>> {
        self.filesystems
            .iter()
            .filter_map(|fs| {
                fs.mount_point.as_ref().map(|mp| MountPointInfo {
                    mount_point: mp,
                    fs_type: fs.fs_type,
                    is_verity: false,
                    device_id: fs.device_id.as_ref(),
                })
            })
            .chain(self.verity_filesystems.iter().map(|vfs| MountPointInfo {
                mount_point: &vfs.mount_point,
                fs_type: vfs.fs_type,
                is_verity: true,
                device_id: Some(&vfs.data_device_id),
            }))
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

    /// Returns a list of tuples (device ID, image) that represent images to be deployed onto the
    /// block devices. Takes in an optional filter function that determines which images to
    /// include.
    pub fn get_images_from_filesystems(
        &self,
        filter: Option<impl Fn(&BlockDeviceId) -> bool>,
    ) -> Vec<(BlockDeviceId, Image)> {
        // If no filter is provided, include all images
        let apply_filter = |device_id: &BlockDeviceId| match &filter {
            Some(f) => f(device_id),
            None => true,
        };

        let mut images = self
            .filesystems
            .iter()
            .filter_map(|fs| fs.device_id.clone().zip(fs.source.image().cloned()))
            .filter(|(device_id, _)| apply_filter(device_id))
            .collect::<Vec<_>>();

        let verity_images = self
            .verity_filesystems
            .iter()
            .flat_map(|vf| {
                let mut imgs = vec![];
                if apply_filter(&vf.data_device_id) {
                    imgs.push((vf.data_device_id.clone(), vf.data_image.clone()));
                }
                if apply_filter(&vf.hash_device_id) {
                    imgs.push((vf.hash_device_id.clone(), vf.hash_image.clone()));
                }
                imgs
            })
            .collect::<Vec<_>>();

        images.extend(verity_images);
        images
    }

    /// Returns a list of tuples (device ID, image) that represent all images on the block devices,
    /// excluding ESP partitions. This includes images on A/B volume pairs and standalone volumes.
    pub fn get_images(&self) -> Vec<(BlockDeviceId, Image)> {
        self.get_images_from_filesystems(None::<fn(&BlockDeviceId) -> bool>)
    }

    /// Returns a list of block device IDs that correspond to the A/B volume pairs.
    pub fn get_ab_volume_pair_ids(&self) -> HashSet<BlockDeviceId> {
        self.ab_update
            .as_ref()
            .map(|ab| ab.volume_pairs.iter().map(|p| p.id.clone()).collect())
            .unwrap_or_default()
    }

    /// Returns a list of tuples (device ID, image) that represent images that need to be deployed onto
    /// A/B volume pairs, based on the host configuration.
    pub fn get_ab_volume_pair_images(&self) -> Vec<(BlockDeviceId, Image)> {
        let ab_volume_pair_ids: HashSet<_> = self.get_ab_volume_pair_ids();

        // Return early if there are no A/B volume pairs
        if ab_volume_pair_ids.is_empty() {
            return vec![];
        }

        // Call get_images_from_filesystems() with a filter that includes only A/B volume pair IDs
        self.get_images_from_filesystems(Some(|device_id: &_| {
            ab_volume_pair_ids.contains(device_id)
        }))
    }

    /// Returns a list of tuples (device ID, image) that represent the ESP images on the ESP
    /// partitions.
    pub fn get_esp_images(&self) -> Vec<(BlockDeviceId, Image)> {
        self.filesystems
            .iter()
            .filter_map(|fs| fs.device_id.clone().zip(fs.source.esp_image().cloned()))
            .collect::<Vec<_>>()
    }

    /// INTERNAL FUNCTION!
    ///
    /// Find the mount point that is holding the given path. This is useful to find
    /// the volume on which the given absolute path is located. This version uses HC
    /// to find the information and is useful early in the process when HS has not
    /// yet been populated.
    pub fn path_to_mount_point<'a>(&'a self, path: &Path) -> Option<&'a InternalMountPoint> {
        self.internal_mount_points
            .iter()
            .filter(|mp| path.starts_with(&mp.path))
            .max_by_key(|mp| mp.path.as_os_str().len())
    }

    /// INTERNAL FUNCTION!
    ///
    /// Returns the mount point and relative path for a given path.
    ///
    /// The mount point is the closest parent directory of the path that is a
    /// mount point. The relative path is the path relative to the mount point.
    pub fn get_mount_point_and_relative_path<'a, 'b>(
        &'a self,
        path: &'b Path,
    ) -> Option<(&'a InternalMountPoint, &'b Path)> {
        self.internal_mount_points
            .iter()
            .filter(|mp| path.starts_with(&mp.path))
            .max_by_key(|mp| mp.path.components().count())
            .and_then(|mp| {
                let rel_path = path.strip_prefix(&mp.path).ok()?;
                Some((mp, rel_path))
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
        constants::{BOOT_MOUNT_POINT_PATH, ROOT_MOUNT_POINT_PATH},
    };

    use self::{
        disks::PartitionTableType,
        encryption::EncryptedVolume,
        filesystem::{FileSystemSource, FileSystemType, MountOptions, MountPoint},
        imaging::{AbVolumePair, Image, ImageFormat, ImageSha256},
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
                    fs_type: FileSystemType::Vfat,
                    source: FileSystemSource::EspImage(Image {
                        url: "file:///esp.raw.zst".to_owned(),
                        sha256: ImageSha256::Ignored,
                        format: ImageFormat::RawZst,
                    }),
                    mount_point: Some(MountPoint {
                        path: PathBuf::from(ESP_MOUNT_POINT_PATH),
                        options: MountOptions::empty(),
                    }),
                },
                FileSystem {
                    device_id: Some("boot".into()),
                    fs_type: FileSystemType::Ext4,
                    source: FileSystemSource::Image(Image {
                        url: "file:///boot.raw.zst".to_owned(),
                        sha256: ImageSha256::Ignored,
                        format: ImageFormat::RawZst,
                    }),
                    mount_point: Some(MountPoint {
                        path: PathBuf::from(BOOT_MOUNT_POINT_PATH),
                        options: MountOptions::empty(),
                    }),
                },
                FileSystem {
                    device_id: Some("root".into()),
                    fs_type: FileSystemType::Ext4,
                    source: FileSystemSource::Image(Image {
                        url: "file:///root.raw.zst".to_owned(),
                        sha256: ImageSha256::Ignored,
                        format: ImageFormat::RawZst,
                    }),
                    mount_point: Some(MountPoint {
                        path: PathBuf::from(ROOT_MOUNT_POINT_PATH),
                        options: MountOptions::empty(),
                    }),
                },
                FileSystem {
                    device_id: Some("srv".into()),
                    fs_type: FileSystemType::Ext4,
                    source: FileSystemSource::Create,
                    mount_point: Some(MountPoint {
                        path: PathBuf::from("/srv"),
                        options: MountOptions::empty(),
                    }),
                },
                FileSystem {
                    device_id: Some("overlay".into()),
                    fs_type: FileSystemType::Ext4,
                    source: FileSystemSource::Create,
                    mount_point: Some(MountPoint {
                        path: PathBuf::from(TRIDENT_OVERLAY_PATH),
                        options: MountOptions::empty(),
                    }),
                },
                FileSystem {
                    device_id: Some("mnt".into()),
                    fs_type: FileSystemType::Ext4,
                    source: FileSystemSource::Create,
                    mount_point: Some(MountPoint {
                        path: PathBuf::from("/mnt"),
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
        storage.verity_filesystems = vec![VerityFileSystem {
            name: "root".into(),
            data_device_id: "root-a".into(),
            hash_device_id: "root-a-verity".into(),
            data_image: Image {
                url: "file:///root.raw.zst".into(),
                sha256: ImageSha256::Ignored,
                format: ImageFormat::RawZst,
            },
            hash_image: Image {
                url: "file:///root-verity.raw.zst".into(),
                sha256: ImageSha256::Ignored,
                format: ImageFormat::RawZst,
            },
            fs_type: FileSystemType::Ext4,
            mount_point: MountPoint {
                path: PathBuf::from(ROOT_MOUNT_POINT_PATH),
                options: MountOptions::new("ro"),
            },
        }];

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
                    fs_type: FileSystemType::Vfat,
                    source: FileSystemSource::EspImage(Image {
                        url: "http://example.com/image".to_string(),
                        sha256: ImageSha256::Ignored,
                        format: ImageFormat::RawZst,
                    }),
                    mount_point: Some(MountPoint {
                        path: PathBuf::from(ESP_MOUNT_POINT_PATH),
                        options: MountOptions::empty(),
                    }),
                },
                FileSystem {
                    device_id: Some("disk1-partition2".to_string()),
                    fs_type: FileSystemType::Ext4,
                    source: FileSystemSource::Image(Image {
                        url: "http://example.com/image".to_string(),
                        sha256: ImageSha256::Ignored,
                        format: ImageFormat::RawZst,
                    }),
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
                volume_pairs: vec![imaging::AbVolumePair {
                    id: "ab-update-volume-pair".to_string(),
                    volume_a_id: "disk1-partition2".to_string(),
                    volume_b_id: "disk2-partition2".to_string(),
                }],
            }),
            filesystems: vec![
                FileSystem {
                    device_id: Some("ab-update-volume-pair".to_string()),
                    fs_type: FileSystemType::Ext4,
                    source: FileSystemSource::Image(Image {
                        url: "http://example.com/image".to_string(),
                        sha256: ImageSha256::Ignored,
                        format: ImageFormat::RawZst,
                    }),
                    mount_point: Some(MountPoint {
                        path: PathBuf::from(ROOT_MOUNT_POINT_PATH),
                        options: MountOptions::empty(),
                    }),
                },
                FileSystem {
                    device_id: Some("disk1-partition1".to_string()),
                    fs_type: FileSystemType::Vfat,
                    source: FileSystemSource::EspImage(Image {
                        url: "http://example.com/image".to_string(),
                        sha256: ImageSha256::Ignored,
                        format: ImageFormat::RawZst,
                    }),
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
            HostConfigurationStaticValidationError::InvalidBlockDeviceGraph(
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
            HostConfigurationStaticValidationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::DuplicateDeviceId("disk1".into())
            )
        );

        let bad_filesystem_target = Storage {
            filesystems: vec![FileSystem {
                device_id: Some("disk99".to_string()),
                fs_type: FileSystemType::Ext4,
                source: FileSystemSource::Image(Image {
                    url: "http://example.com/image".to_string(),
                    sha256: ImageSha256::Ignored,
                    format: ImageFormat::RawZst,
                }),
                mount_point: Some(MountPoint {
                    path: PathBuf::from("/some/path"),
                    options: MountOptions::empty(),
                }),
            }],
            ..storage.clone()
        };
        assert_eq!(
            bad_filesystem_target.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::FilesystemNonExistentReference {
                    target_id: "disk99".into(),
                    fs_desc: bad_filesystem_target.filesystems[0].description()
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
                    fs_type: FileSystemType::Vfat,
                    source: FileSystemSource::EspImage(Image {
                        url: "https://some/url".to_owned(),
                        sha256: imaging::ImageSha256::Checksum("".into()),
                        format: ImageFormat::RawZst,
                    }),
                    mount_point: Some(MountPoint {
                        path: PathBuf::from(ESP_MOUNT_POINT_PATH),
                        options: MountOptions::empty(),
                    }),
                },
                FileSystem {
                    device_id: Some("ab1".to_owned()),
                    fs_type: FileSystemType::Ext4,
                    source: FileSystemSource::Image(Image {
                        url: "https://some/url".to_owned(),
                        sha256: imaging::ImageSha256::Checksum("".into()),
                        format: ImageFormat::RawZst,
                    }),
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

        // fail on duplicate id
        storage = storage_golden.clone();
        storage.disks.get_mut(0).unwrap().partitions = vec![Partition {
            id: "part1".to_owned(),
            partition_type: PartitionType::Esp,
            size: PartitionSize::from_str("1M").unwrap(),
        }];
        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::DuplicateDeviceId("part1".into())
            ),
        );

        // fail on duplicate id
        storage = storage_golden.clone();
        storage.ab_update.as_mut().unwrap().volume_pairs[0].id = "disk1".to_owned();
        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::DuplicateDeviceId("disk1".into())
            ),
        );

        // fail on missing reference (disk4 does not exist)
        storage = storage_golden.clone();
        storage.ab_update.as_mut().unwrap().volume_pairs[0].volume_a_id = "disk4".to_owned();
        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::NonExistentReference {
                    node_id: "ab1".into(),
                    kind: BlkDevKind::ABVolume,
                    target_id: "disk4".into()
                }
            ),
        );

        // fail on missing reference (disk4 does not exist)
        storage = storage_golden.clone();
        storage.filesystems[0].device_id = Some("disk4".into());
        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::FilesystemNonExistentReference {
                    target_id: "disk4".into(),
                    fs_desc: storage.filesystems[0].description(),
                }
            ),
        );

        // fail on bad block device type
        storage = storage_golden.clone();
        storage.filesystems[0].device_id = Some("disk1".into());
        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::FilesystemInvalidReference {
                    fs_desc: storage.filesystems[0].description(),
                    target_id: "disk1".into(),
                    target_kind: BlkDevKind::Disk,
                    valid_references: BlkDevReferrerKind::FileSystemEsp.valid_target_kinds()
                }
            ),
        );

        // fail if devices are not all the same size for a RAID
        storage = storage_golden.clone();
        storage.disks[1].partitions[3].size = PartitionSize::from_str("2G").unwrap();
        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::PartitionSizeMismatch {
                    node_id: "my-raid1".into(),
                    kind: BlkDevKind::RaidArray
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
            HostConfigurationStaticValidationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::BasicCheckFailed {
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
            HostConfigurationStaticValidationError::InvalidBlockDeviceGraph(
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
            HostConfigurationStaticValidationError::InvalidBlockDeviceGraph(
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
            HostConfigurationStaticValidationError::InvalidBlockDeviceGraph(
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
            HostConfigurationStaticValidationError::InvalidBlockDeviceGraph(
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
            HostConfigurationStaticValidationError::InvalidBlockDeviceGraph(
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
            HostConfigurationStaticValidationError::InvalidBlockDeviceGraph(
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
                device_id: "alt-enc".to_owned(),
            });

        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidBlockDeviceGraph(
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
                device_id: "alt-enc".to_owned(),
            });
        storage.filesystems.push(FileSystem {
            device_id: Some("alt".to_owned()),
            fs_type: FileSystemType::Ext4,
            source: FileSystemSource::Create,
            mount_point: Some(MountPoint {
                path: PathBuf::from("/alt"),
                options: MountOptions::empty(),
            }),
        });
        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidBlockDeviceGraph(
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
            HostConfigurationStaticValidationError::InvalidEncryptionRecoveryKeyUrlScheme {
                url: "https://www.example.com/recovery.key".into(),
                scheme: "https".into(),
            }
        );
    }

    /// Encrypted volume target ID may be a home partition
    #[test]
    fn test_validate_encryption_target_id_home_pass() {
        let mut storage: Storage = get_storage();
        storage.disks[1]
            .partitions
            .iter_mut()
            .find(|p| p.id == "srv-enc")
            .unwrap()
            .partition_type = PartitionType::Home;
        storage.validate(true).unwrap();
    }

    /// Encrypted volume target ID must not be an esp partition
    #[test]
    fn test_validate_encryption_target_id_esp_fail() {
        let mut storage: Storage = get_storage();
        // Remoce the filesystem associated with esp
        storage
            .filesystems
            .retain(|fs| fs.device_id != Some("esp".into()));

        // Update the target ID of the encrypted volume to esp
        storage.encryption.as_mut().unwrap().volumes[0].device_id = "esp".to_owned();

        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::InvalidPartitionType {
                    node_id: "srv".into(),
                    kind: BlkDevKind::EncryptedVolume,
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
            HostConfigurationStaticValidationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::InvalidPartitionType {
                    node_id: "alt".into(),
                    kind: BlkDevKind::EncryptedVolume,
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
            HostConfigurationStaticValidationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::InvalidPartitionType {
                    node_id: "srv".into(),
                    kind: BlkDevKind::EncryptedVolume,
                    partition_id: "root-b-verity".into(),
                    partition_type: PartitionType::RootVerity,
                    valid_types: BlkDevReferrerKind::EncryptedVolume.allowed_partition_types()
                }
            ),
            "Block device 'srv' of kind 'encrypted volume' references invalid targets"
        );
    }

    /// Encrypted volume target ID may be a software RAID array of home partitions
    #[test]
    fn test_validate_encryption_target_id_raid_home_pass() {
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
                p.partition_type = PartitionType::Home;
            });

        storage.validate(true).unwrap();
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
            HostConfigurationStaticValidationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::InvalidPartitionType {
                    node_id: "srv".into(),
                    kind: BlkDevKind::EncryptedVolume,
                    partition_id: "mnt-raid-1".into(),
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
            HostConfigurationStaticValidationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::InvalidPartitionType {
                    node_id: "srv".into(),
                    kind: BlkDevKind::EncryptedVolume,
                    partition_id: "mnt-raid-1".into(),
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
            HostConfigurationStaticValidationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::InvalidPartitionType {
                    node_id: "srv".into(),
                    kind: BlkDevKind::EncryptedVolume,
                    partition_id: "mnt-raid-1".into(),
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
            HostConfigurationStaticValidationError::InvalidBlockDeviceGraph(
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
        storage.encryption.as_mut().unwrap().volumes[0].device_id = "mnt".to_owned();
        storage.raid.software[0].devices = vec!["root".to_owned()];
        // Remove the first mount point
        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidBlockDeviceGraph(
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
        storage.encryption.as_mut().unwrap().volumes[0].device_id = "disk1".to_owned();
        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidBlockDeviceGraph(
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
            HostConfigurationStaticValidationError::InvalidBlockDeviceGraph(
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
                device_id: "srv-enc".to_owned(),
            });
        storage.filesystems.push(FileSystem {
            device_id: Some("alt".to_owned()),
            fs_type: FileSystemType::Ext4,
            source: FileSystemSource::Create,
            mount_point: Some(MountPoint {
                path: PathBuf::from("/alt"),
                options: MountOptions::empty(),
            }),
        });
        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidBlockDeviceGraph(
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

    /// Encrypted volumes cannot target the same partition as a filesystem/mount point
    #[test]
    fn test_validate_encryption_mount_point_target_part_id_equal_fail() {
        let mut storage: Storage = get_storage();

        // Add a new filesystem to the partition used for encryption
        storage.filesystems.push(FileSystem {
            device_id: Some("srv-enc".to_owned()),
            fs_type: FileSystemType::Ext4,
            source: FileSystemSource::Create,
            mount_point: Some(MountPoint {
                path: PathBuf::from("/mnt/some-mount-point"),
                options: MountOptions::empty(),
            }),
        });

        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::ReferrerForbiddenSharing {
                    target_id: "srv-enc".into(),
                    target_kind: BlkDevKind::Partition,
                    referrer_a_id: "ext4 filesystem mounted at /mnt/some-mount-point".into(),
                    referrer_a_kind: BlkDevReferrerKind::FileSystem,
                    referrer_a_valid_sharing_peers: BlkDevReferrerKind::FileSystem
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
        storage.encryption.as_mut().unwrap().volumes[0].device_id = "mnt-raid-1".to_owned();
        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidBlockDeviceGraph(
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
        storage.encryption.as_mut().unwrap().volumes[0].device_id = "root-a".to_owned();
        storage.disks[1].partitions[1].partition_type = PartitionType::LinuxGeneric;
        storage.disks[1].partitions[3].partition_type = PartitionType::LinuxGeneric;
        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidBlockDeviceGraph(
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
        storage.encryption.as_mut().unwrap().volumes[0].device_id = "root-b".to_owned();
        storage.disks[1].partitions[1].partition_type = PartitionType::LinuxGeneric;
        storage.disks[1].partitions[3].partition_type = PartitionType::LinuxGeneric;
        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidBlockDeviceGraph(
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
            HostConfigurationStaticValidationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::ReferrerForbiddenSharing {
                    target_id: "alt-a-enc".into(),
                    target_kind: BlkDevKind::Partition,
                    referrer_a_id: "alt".into(),
                    referrer_a_kind: BlkDevReferrerKind::ABVolume,
                    referrer_a_valid_sharing_peers: BlkDevReferrerKind::ABVolume
                        .valid_sharing_peers(),
                    referrer_b_id: "alt-a".into(),
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
            HostConfigurationStaticValidationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::ReferrerForbiddenSharing {
                    target_id: "alt-b-enc".into(),
                    target_kind: BlkDevKind::Partition,
                    referrer_a_id: "alt".into(),
                    referrer_a_kind: BlkDevReferrerKind::ABVolume,
                    referrer_a_valid_sharing_peers: BlkDevReferrerKind::ABVolume
                        .valid_sharing_peers(),
                    referrer_b_id: "alt-b".into(),
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

        // Change the image format to raw-lzma in the root filesystem
        storage
            .filesystems
            .iter_mut()
            .find(|fs| fs.device_id == Some("root".into()))
            .unwrap()
            .source = FileSystemSource::Image(Image {
            url: "file:///root.raw.lzma".into(),
            sha256: ImageSha256::Ignored,
            format: ImageFormat::RawLzma,
        });

        storage.validate(true).unwrap();
    }

    /// Image must not be a partition if format is raw-lzma
    #[test]
    #[cfg(feature = "sysupdate")]
    fn test_validate_image_raw_lzma_partition_fail() {
        let mut storage: Storage = get_storage();

        // Change the image format to raw-lzma in the esp filesystem
        storage.filesystems[0].source = FileSystemSource::Image(Image {
            url: "file:///esp.raw.lzma".into(),
            sha256: ImageSha256::Ignored,
            format: ImageFormat::RawLzma,
        });

        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::InvalidBlockDeviceGraph(
                BlockDeviceGraphBuildError::FilesystemInvalidReference {
                    fs_desc: storage.filesystems[0].description(),
                    target_id: "esp".into(),
                    target_kind: BlkDevKind::Partition,
                    valid_references: BlkDevReferrerKind::FileSystemSysupdate.valid_target_kinds()
                },
            )
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
            .source = FileSystemSource::Image(Image {
            url: "file:///srv.raw.zst".to_owned(),
            sha256: ImageSha256::Ignored,
            format: ImageFormat::RawZst,
        });

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
        storage.verity_filesystems[0].mount_point.options = MountOptions::empty();

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
        storage.verity_filesystems[0].name = "verity-root-a".into();

        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::RootVerityDeviceNameInvalid {
                device_name: "verity-root-a".into()
            }
        );
    }

    #[test]
    fn test_validate_verity_without_boot_image_fail() {
        let mut storage: Storage = get_verity_storage();

        // Change the boot fs to create instead of image
        storage.filesystems[1].source = FileSystemSource::Create;

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
            .options = MountOptions::new("ro");

        assert_eq!(
            storage.validate(true).unwrap_err(),
            HostConfigurationStaticValidationError::OverlayOnReadOnlyVolume {
                overlay_path: "/var/lib/trident-overlay".into(),
                mount_point_path: "/var/lib/trident-overlay".into(),
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
                        devices: vec!["part3".to_owned(), "part4".to_owned()],
                    }],
                    ..Default::default()
                },
                filesystems: vec![FileSystem {
                    device_id: Some("part1".to_owned()),
                    fs_type: FileSystemType::Vfat,
                    source: FileSystemSource::EspImage(Image {
                        url: "".to_owned(),
                        sha256: ImageSha256::Ignored,
                        format: ImageFormat::RawZst,
                    }),
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
            fs_type: FileSystemType::Ext4,
            source: FileSystemSource::Create,
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
                        fs_type: FileSystemType::Vfat,
                        source: FileSystemSource::EspImage(Image {
                            url: "http://example.com/esp_1.img".to_string(),
                            sha256: ImageSha256::Checksum("esp_sha256_1".to_string()),
                            format: ImageFormat::RawZst,
                        }),
                        mount_point: Some(MountPoint {
                            path: PathBuf::from("/esp"),
                            options: MountOptions::empty(),
                        }),
                    },
                    FileSystem {
                        device_id: Some("root".into()),
                        fs_type: FileSystemType::Vfat,
                        source: FileSystemSource::Image(Image {
                            url: "http://example.com/root_1.img".to_string(),
                            sha256: ImageSha256::Checksum("root_sha256_1".to_string()),
                            format: ImageFormat::RawZst,
                        }),
                        mount_point: Some(MountPoint {
                            path: PathBuf::from("/"),
                            options: MountOptions::empty(),
                        }),
                    },
                    FileSystem {
                        device_id: Some("trident".into()),
                        fs_type: FileSystemType::Vfat,
                        source: FileSystemSource::Image(Image {
                            url: "http://example.com/trident_1.img".to_string(),
                            sha256: ImageSha256::Checksum("trident_sha256_1".to_string()),
                            format: ImageFormat::RawZst,
                        }),
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
    fn test_get_mount_point_info_and_relative_path() {
        let host_config = {
            let mut hc = HostConfiguration {
                storage: Storage {
                    filesystems: vec![
                        FileSystem {
                            device_id: Some("root".into()),
                            fs_type: FileSystemType::Ext4,
                            source: FileSystemSource::Create,
                            mount_point: Some(MountPoint {
                                path: PathBuf::from(ROOT_MOUNT_POINT_PATH),
                                options: MountOptions::empty(),
                            }),
                        },
                        FileSystem {
                            device_id: Some("boot".into()),
                            fs_type: FileSystemType::Ext4,
                            source: FileSystemSource::Create,
                            mount_point: Some(MountPoint {
                                path: PathBuf::from(BOOT_MOUNT_POINT_PATH),
                                options: MountOptions::empty(),
                            }),
                        },
                        FileSystem {
                            device_id: Some("efi".into()),
                            fs_type: FileSystemType::Vfat,
                            source: FileSystemSource::Create,
                            mount_point: Some(MountPoint {
                                path: PathBuf::from(ESP_MOUNT_POINT_PATH),
                                options: MountOptions::empty(),
                            }),
                        },
                    ],
                    ..Default::default()
                },
                ..Default::default()
            };

            hc.populate_internal();
            hc
        };

        fn test_fn(
            hc: &HostConfiguration,
            path: &'static str,
            expected_fs_index: usize,
            expected_rp: &str,
        ) {
            println!("Testing path: {}", path);
            println!("Expected filesystem index: {}", expected_fs_index);
            println!("Expected relative path: {}", expected_rp);
            let expected_fs = &hc.storage.filesystems[expected_fs_index];

            // First check new API functions
            let (mpi, rp) = hc
                .storage
                .get_mount_point_info_and_relative_path(Path::new(path))
                .unwrap();
            assert_eq!(mpi.device_id, expected_fs.device_id.as_ref());
            assert_eq!(mpi.mount_point, expected_fs.mount_point.as_ref().unwrap());
            assert_eq!(mpi.fs_type, expected_fs.fs_type);
            assert_eq!(rp, Path::new(expected_rp));

            // Now check old internal functions
            let (mp, rp) = hc
                .storage
                .get_mount_point_and_relative_path(Path::new(path))
                .unwrap();
            assert_eq!(
                mp.target_id,
                expected_fs.device_id.as_deref().unwrap_or_default()
            );
            assert_eq!(
                mp.path,
                expected_fs
                    .mount_point
                    .as_ref()
                    .map(|mp| mp.path.as_path())
                    .unwrap_or(Path::new("none"))
            );
            assert_eq!(mp.filesystem, expected_fs.fs_type);
            assert_eq!(rp, Path::new(expected_rp));
        }

        test_fn(&host_config, "/", 0, "");
        test_fn(&host_config, "/some/random/path", 0, "some/random/path");
        test_fn(&host_config, "/boot/", 1, "");
        test_fn(&host_config, "/boot/efi.cfg", 1, "efi.cfg");
        test_fn(&host_config, "/boot/some/path", 1, "some/path");
        test_fn(&host_config, "/boot/efi", 2, "");
        test_fn(&host_config, "/boot/efi/", 2, "");
        test_fn(&host_config, "/boot/efi/foobar", 2, "foobar");
        test_fn(&host_config, "/boot/efi/foobar/", 2, "foobar");
    }

    /// Validates that get_esp_images() correctly returns a list of images associated with the ESP
    /// filesystems.
    #[test]
    fn test_get_esp_images() {
        // Initialize a basic Storage object.
        let mut storage: Storage = get_storage();

        // Test case #1: Validate that get_esp_images() correctly returns 'esp'.
        let esp_images = storage.get_esp_images();
        assert_eq!(esp_images.len(), 1);
        // Create a tuple of 'esp' and the associated Image object.
        let expected_image = Image {
            url: "file:///esp.raw.zst".to_string(),
            sha256: ImageSha256::Ignored,
            format: ImageFormat::RawZst,
        };
        let expected_tuple = ("esp".to_string(), expected_image.clone());
        // Add the tuple to a list.
        let expected_list = vec![expected_tuple.clone()];
        // Compare the list with esp_images.
        assert_eq!(esp_images, expected_list);

        // Test case #2: Add another ESP filesystem and confirm that get_esp_images() now returns
        // both 'esp' and 'esp1'.
        // Create a new ESP filesystem.
        let esp1 = FileSystem {
            device_id: Some("esp1".to_owned()),
            fs_type: FileSystemType::Vfat,
            source: FileSystemSource::EspImage(Image {
                url: "file:///esp1.raw.zst".to_owned(),
                sha256: ImageSha256::Ignored,
                format: ImageFormat::RawZst,
            }),
            mount_point: Some(MountPoint {
                path: PathBuf::from("/esp1"),
                options: MountOptions::empty(),
            }),
        };
        // Add the new ESP filesystem to the storage object.
        storage.filesystems.push(esp1);
        // Call get_esp_images() and confirm that it returns both 'esp' and 'esp1'.
        let esp_images = storage.get_esp_images();
        assert_eq!(esp_images.len(), 2);
        // Create a tuple of 'esp1' and the associated Image object.
        let expected_image1 = Image {
            url: "file:///esp1.raw.zst".to_string(),
            sha256: ImageSha256::Ignored,
            format: ImageFormat::RawZst,
        };
        let expected_tuple1 = ("esp1".to_string(), expected_image1.clone());
        // Add the tuple to a list.
        let expected_list1 = vec![expected_tuple.clone(), expected_tuple1.clone()];
        // Compare the list with esp_images.
        assert_eq!(esp_images, expected_list1);

        // Test case #3: Add a new ESP filesystem without an associated Image object and confirm
        // that get_esp_images() still returns both 'esp' and 'esp1'.
        // Create a new ESP filesystem without an associated Image object.
        let esp2 = FileSystem {
            device_id: Some("esp2".to_owned()),
            fs_type: FileSystemType::Vfat,
            source: FileSystemSource::Create,
            mount_point: Some(MountPoint {
                path: PathBuf::from("/esp2"),
                options: MountOptions::empty(),
            }),
        };
        // Add the new ESP filesystem to the storage object.
        storage.filesystems.push(esp2);
        // Call get_esp_images() and confirm that it still returns both 'esp' and 'esp1'.
        let esp_images = storage.get_esp_images();
        assert_eq!(esp_images.len(), 2);
        assert_eq!(esp_images, expected_list1);

        // Test case #4: Remove all ESP filesystems and confirm that get_esp_images() returns an empty list.
        // Remove all ESP filesystems from the storage object.
        storage.filesystems.retain(|fs| {
            fs.device_id != Some("esp".to_owned()) && fs.device_id != Some("esp1".to_owned())
        });
        // Call get_esp_images() and confirm that it returns an empty list.
        let esp_images = storage.get_esp_images();
        assert_eq!(esp_images.len(), 0);
    }

    /// Validates that get_images_from_filesystems() correctly returns a list of images associated
    /// with the filesystems, based on an optional filter.
    #[test]
    fn test_get_images_from_filesystems() {
        // Initialize a basic Storage object.
        let storage: Storage = get_storage();

        // Test case #1: Validate that get_images_from_filesystems() correctly returns all images
        // for non-ESP volumes when no filter is applied. This can be confirmed by calling
        // get_images().
        let all_images = storage.get_images_from_filesystems(None::<fn(&BlockDeviceId) -> bool>);
        let expected_images = vec![
            (
                "boot".to_string(),
                Image {
                    url: "file:///boot.raw.zst".to_owned(),
                    sha256: ImageSha256::Ignored,
                    format: ImageFormat::RawZst,
                },
            ),
            (
                "root".to_string(),
                Image {
                    url: "file:///root.raw.zst".to_owned(),
                    sha256: ImageSha256::Ignored,
                    format: ImageFormat::RawZst,
                },
            ),
        ];
        assert_eq!(all_images.len(), 2);
        assert_eq!(all_images, expected_images);

        // Test case #2: Validate that get_images_from_filesystems() correctly returns when a
        // filter is applied on device_id.
        let root_images =
            storage.get_images_from_filesystems(Some(|device_id: &_| device_id == "root"));
        assert_eq!(root_images.len(), 1);
        assert_eq!(
            root_images,
            vec![(
                "root".to_string(),
                Image {
                    url: "file:///root.raw.zst".to_owned(),
                    sha256: ImageSha256::Ignored,
                    format: ImageFormat::RawZst,
                },
            ),]
        );

        // Test case #3: Validate that get_images_from_filesystems() correctly returns an empty
        // list when the filter does not match any filesystems.
        let no_images =
            storage.get_images_from_filesystems(Some(|device_id: &_| device_id == "non-existent"));
        assert_eq!(no_images.len(), 0);

        // Test case #4: Validate that get_images_from_filesystems() correctly returns image 'root'
        // when the filter is applied to select only A/B volumes. This can be confirmed by calling
        // get_ab_volume_pair_images() and get_ab_volume_pair_ids().
        let ab_volume_pair_images = storage.get_ab_volume_pair_images();
        assert_eq!(root_images.len(), 1);
        assert_eq!(
            ab_volume_pair_images,
            vec![(
                "root".to_string(),
                Image {
                    url: "file:///root.raw.zst".to_owned(),
                    sha256: ImageSha256::Ignored,
                    format: ImageFormat::RawZst,
                },
            ),]
        );
        let ab_volume_pair_ids = storage.get_ab_volume_pair_ids();
        assert_eq!(ab_volume_pair_ids.len(), 1);
        assert_eq!(
            ab_volume_pair_ids,
            HashSet::from_iter(vec!["root".to_string()])
        );

        // Test case #5: Validates that when ab_update is None, get_ab_volume_pair_images() and
        // get_ab_volume_pair_ids() should return an empty list.
        let mut storage_no_ab_update: Storage = get_storage();
        storage_no_ab_update.ab_update = None;
        let ab_volume_pair_images = storage_no_ab_update.get_ab_volume_pair_images();
        assert_eq!(ab_volume_pair_images.len(), 0);
        let ab_volume_pair_ids = storage_no_ab_update.get_ab_volume_pair_ids();
        assert_eq!(ab_volume_pair_ids.len(), 0);
    }
}
