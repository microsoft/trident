use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Error};
use filesystem::FileSystemData;
use log::{debug, trace};

use trident_api::{
    config::{HostConfiguration, Partition, VerityDevice},
    constants::{internal_params::ENABLE_UKI_SUPPORT, ROOT_MOUNT_POINT_PATH},
    error::TridentError,
    status::{AbVolumeSelection, ServicingType},
    storage_graph::graph::StorageGraph,
    BlockDeviceId,
};

use crate::osimage::OsImage;

#[allow(dead_code)]
pub mod filesystem;

#[cfg(test)]
mod test_utils;

/// Helper struct to consolidate the info on the A/B volume pair. Contains the paths and block
/// device IDs for both volumes.
#[derive(Debug, PartialEq)]
pub(crate) struct AbVolumePairInfo {
    pub volume_a_path: PathBuf,
    pub volume_b_path: PathBuf,
    pub volume_a_id: BlockDeviceId,
    pub volume_b_id: BlockDeviceId,
}

#[cfg_attr(any(test, feature = "functional-test"), derive(Clone, Default))]
pub struct EngineContext {
    pub spec: HostConfiguration,

    pub spec_old: HostConfiguration,

    /// Type of servicing that Trident is executing on the host.
    pub servicing_type: ServicingType,

    /// The path associated with each partition in the Host Configuration.
    pub partition_paths: BTreeMap<BlockDeviceId, PathBuf>,

    /// A/B update status.
    pub ab_active_volume: Option<AbVolumeSelection>,

    /// Stores the Disks UUID to ID mapping of the host.
    pub disk_uuids: HashMap<BlockDeviceId, uuid::Uuid>,

    /// Index of the current Azure Linux install. Used to distinguish between
    /// different installs of Azure Linux on the same host.
    ///
    /// An AzL "install" is the result of a deployment of Azure Linux (e.g. with
    /// Trident), and encompasses the entire deployment, including both A/B
    /// volumes (when present).
    ///
    /// Indexes are assigned sequentially, starting from 0. On a clean install,
    /// Trident will determine the next available index and use it for the new
    /// install.
    pub install_index: usize,

    /// The OS image that Trident is using to service the host.
    pub image: Option<OsImage>,

    /// The storage graph representing the storage configuration of the host.
    pub storage_graph: StorageGraph,

    /// All of the filesystems in the system.
    pub filesystems: Vec<FileSystemData>,

    /// Whether the image will use a UKI or not.
    pub is_uki: Option<bool>,
}
impl EngineContext {
    /// Returns the update volume selection for all A/B volume pairs. The update volume is the one
    /// that is meant to be updated, based on the servicing in progress, if any.
    pub fn get_ab_update_volume(&self) -> Option<AbVolumeSelection> {
        match self.servicing_type {
            // If there is no servicing in progress, update volume is None.
            ServicingType::NoActiveServicing => None,
            // If host is executing a "normal" update, active and update volumes are the same.
            ServicingType::HotPatch
            | ServicingType::NormalUpdate
            | ServicingType::UpdateAndReboot => self.ab_active_volume,
            // If host is executing an A/B update, update volume is the opposite of active volume.
            ServicingType::AbUpdate => {
                if self.ab_active_volume == Some(AbVolumeSelection::VolumeA) {
                    Some(AbVolumeSelection::VolumeB)
                } else {
                    Some(AbVolumeSelection::VolumeA)
                }
            }
            // If host is executing a clean install, update volume is always A.
            ServicingType::CleanInstall => Some(AbVolumeSelection::VolumeA),
        }
    }

    /// Using the `/` mount point, fetches the root block device ID.
    pub(super) fn get_root_block_device_id(&self) -> Option<BlockDeviceId> {
        self.spec
            .storage
            .path_to_filesystem(ROOT_MOUNT_POINT_PATH)
            .and_then(|f| f.device_id.clone())
    }

    /// Using the `/` mount point, fetches the root block device path.
    pub(super) fn get_root_block_device_path(&self) -> Option<PathBuf> {
        self.get_root_block_device_id()
            .and_then(|id| self.get_block_device_path(&id))
    }

    /// Returns the path of the block device with id `block_device_id`.
    ///
    /// If the volume is part of an A/B volume pair, this function returns the update volume, i.e.
    /// the one that isn't active.
    pub(crate) fn get_block_device_path(&self, block_device_id: &BlockDeviceId) -> Option<PathBuf> {
        if let Some(partition_path) = self.partition_paths.get(block_device_id) {
            return Some(partition_path.clone());
        }

        if let Some(raid) = self
            .spec
            .storage
            .raid
            .software
            .iter()
            .find(|r| &r.id == block_device_id)
        {
            return Some(raid.device_path());
        }

        if let Some(encryption) = &self.spec.storage.encryption {
            if let Some(encrypted) = encryption.volumes.iter().find(|e| &e.id == block_device_id) {
                return Some(encrypted.device_path());
            }
        }

        if let Some(verity) = self
            .spec
            .storage
            .verity
            .iter()
            .find(|v| &v.id == block_device_id)
        {
            return Some(verity.device_path());
        }

        self.get_ab_volume_block_device_id(block_device_id)
            .and_then(|child_block_device_id| self.get_block_device_path(child_block_device_id))
    }

    /// Returns the block device id for the update volume from the given A/B volume pair.
    pub(super) fn get_ab_volume_block_device_id(
        &self,
        block_device_id: &BlockDeviceId,
    ) -> Option<&BlockDeviceId> {
        if let Some(ab_update) = &self.spec.storage.ab_update {
            let ab_volume = ab_update
                .volume_pairs
                .iter()
                .find(|v| &v.id == block_device_id);
            if let Some(v) = ab_volume {
                let selection = self.get_ab_update_volume();
                // Return the appropriate BlockDeviceId based on the selection
                return selection.map(|sel| match sel {
                    AbVolumeSelection::VolumeA => &v.volume_a_id,
                    AbVolumeSelection::VolumeB => &v.volume_b_id,
                });
            };
        }
        None
    }

    /// Returns A/B volume pair info based on a given block device ID.
    pub(crate) fn get_ab_volume_pair(
        &self,
        device_id: &BlockDeviceId,
    ) -> Result<AbVolumePairInfo, Error> {
        let ab_volume_pair = self
            .spec
            .storage
            .ab_update
            .as_ref()
            .context("No A/B update configuration found")?
            .volume_pairs
            .iter()
            .find(|p| &p.id == device_id)
            .context(format!(
                "No volume pair for block device ID '{device_id}' found"
            ))?;

        debug!(
            "A/B volume pair with block device ID '{}': {:?}",
            device_id, ab_volume_pair
        );

        let volume_a_path = self
            .get_block_device_path(&ab_volume_pair.volume_a_id)
            .context(format!(
                "Failed to get block device path for volume A with ID '{}'",
                ab_volume_pair.volume_a_id
            ))?;
        let volume_b_path = self
            .get_block_device_path(&ab_volume_pair.volume_b_id)
            .context(format!(
                "Failed to get block device path for volume B with ID '{}'",
                ab_volume_pair.volume_b_id
            ))?;

        Ok(AbVolumePairInfo {
            volume_a_path,
            volume_b_path,
            volume_a_id: ab_volume_pair.volume_a_id.clone(),
            volume_b_id: ab_volume_pair.volume_b_id.clone(),
        })
    }

    /// Returns the configuration for the verity device for the given block device ID.
    pub(crate) fn get_verity_config(
        &self,
        device_id: &BlockDeviceId,
    ) -> Result<VerityDevice, Error> {
        let verity_device_config = self
            .spec
            .storage
            .verity
            .iter()
            .find(|vd| &vd.id == device_id)
            .cloned()
            .context(format!(
                "Failed to find configuration for verity device '{device_id}'"
            ))?;

        trace!(
            "Config for verity device '{}': {:?}",
            device_id,
            verity_device_config
        );

        Ok(verity_device_config)
    }

    /// Returns the first partition that backs the given block device, or Err if the block device ID
    /// does not correspond to a partition or software RAID array.
    pub(crate) fn get_first_backing_partition<'a>(
        &'a self,
        block_device_id: &BlockDeviceId,
    ) -> Result<&'a Partition, Error> {
        if let Some(partition) = self.spec.storage.get_partition(block_device_id) {
            Ok(partition)
        } else if let Some(array) = self
            .spec
            .storage
            .raid
            .software
            .iter()
            .find(|r| &r.id == block_device_id)
        {
            let partition_id = array
                .devices
                .first()
                .context(format!("RAID array '{}' has no partitions", array.id))?;

            self.spec
                .storage
                .get_partition(partition_id)
                .context(format!(
                    "RAID array '{block_device_id}' doesn't reference partition"
                ))
        } else {
            bail!("Block device '{block_device_id}' is not a partition or RAID array")
        }
    }

    /// Returns the estimated size of the block device holding the filesystem that contains the
    /// given path. If the path is not mounted anywhere, or if the block device size cannot be
    /// estimated, returns None.
    pub(crate) fn filesystem_block_device_size(&self, path: impl AsRef<Path>) -> Option<u64> {
        let device = self
            .spec
            .storage
            .path_to_mount_point_info(path)
            .and_then(|mp| mp.device_id)?;

        self.storage_graph.block_device_size(device)
    }

    pub(crate) fn is_uki_image(&self) -> Result<bool, TridentError> {
        if self.spec.internal_params.get_flag(ENABLE_UKI_SUPPORT) {
            trace!("internal param {ENABLE_UKI_SUPPORT} specified: UKI image");
            Ok(true)
        } else if let Some(is_uki) = self.is_uki {
            trace!("uki configured as {is_uki}");
            Ok(is_uki)
        } else {
            Err(TridentError::internal(
                "is_uki() called without it being set",
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::str::FromStr;

    use const_format::formatcp;
    use maplit::btreemap;

    use osutils::testutils::repart::TEST_DISK_DEVICE_PATH;

    use trident_api::config::{
        self, AbUpdate, AbVolumePair, Disk, FileSystem, FileSystemSource, HostConfiguration,
        MountOptions, MountPoint, Partition, PartitionSize, PartitionType, Raid, RaidLevel,
        SoftwareRaidArray, Storage, VerityDevice,
    };

    #[test]
    fn test_get_root_block_device_path() {
        let ctx = EngineContext {
            spec: HostConfiguration {
                storage: config::Storage {
                    disks: vec![Disk {
                        id: "foo".to_owned(),
                        device: PathBuf::from("/dev/sda"),
                        partitions: vec![
                            Partition {
                                id: "boot".to_owned(),
                                size: 2.into(),
                                partition_type: PartitionType::Esp,
                            },
                            Partition {
                                id: "root".to_owned(),
                                size: 7.into(),
                                partition_type: PartitionType::Root,
                            },
                        ],
                        ..Default::default()
                    }],
                    filesystems: vec![
                        FileSystem {
                            device_id: Some("boot".to_owned()),
                            mount_point: Some(MountPoint {
                                path: PathBuf::from("/boot"),
                                options: MountOptions::empty(),
                            }),
                            source: FileSystemSource::Image,
                        },
                        FileSystem {
                            device_id: Some("root".to_owned()),
                            mount_point: Some(MountPoint {
                                path: PathBuf::from(ROOT_MOUNT_POINT_PATH),
                                options: MountOptions::empty(),
                            }),
                            source: FileSystemSource::Image,
                        },
                    ],
                    ..Default::default()
                },
                ..Default::default()
            },
            partition_paths: btreemap! {
                "foo".to_owned() => PathBuf::from("/dev/sda"),
                "boot".to_owned() => PathBuf::from("/dev/sda1"),
                "root".to_owned() => PathBuf::from("/dev/sda2"),
            },
            ..Default::default()
        };

        assert_eq!(
            ctx.get_root_block_device_path(),
            Some(PathBuf::from("/dev/sda2"))
        );
    }

    /// Validates that get_block_device_for_update() works as expected for disks, partitions, and
    /// A/B volumes.
    #[test]
    fn test_get_block_device_for_update() {
        let mut ctx = EngineContext {
            spec: HostConfiguration {
                storage: config::Storage {
                    disks: vec![
                        Disk {
                            id: "os".to_owned(),
                            device: PathBuf::from("/dev/disk/by-bus/foobar"),
                            partitions: vec![
                                Partition {
                                    id: "efi".to_owned(),
                                    size: 100.into(),
                                    partition_type: PartitionType::Esp,
                                },
                                Partition {
                                    id: "root".to_owned(),
                                    size: 900.into(),
                                    partition_type: PartitionType::Root,
                                },
                                Partition {
                                    id: "rootb".to_owned(),
                                    size: 9000.into(),
                                    partition_type: PartitionType::Root,
                                },
                            ],
                            ..Default::default()
                        },
                        Disk {
                            id: "data".to_owned(),
                            device: PathBuf::from("/dev/disk/by-bus/foobar"),
                            partitions: vec![],
                            ..Default::default()
                        },
                    ],
                    ab_update: Some(AbUpdate {
                        volume_pairs: vec![AbVolumePair {
                            id: "osab".to_string(),
                            volume_a_id: "root".to_string(),
                            volume_b_id: "rootb".to_string(),
                        }],
                    }),
                    ..Default::default()
                },
                ..Default::default()
            },
            partition_paths: btreemap! {
                "os".to_owned() => PathBuf::from("/dev/disk/by-bus/foobar"),
                "efi".to_owned() => PathBuf::from("/dev/disk/by-partlabel/osp1"),
                "root".to_owned() => PathBuf::from("/dev/disk/by-partlabel/osp2"),
                "rootb".to_owned() => PathBuf::from("/dev/disk/by-partlabel/osp3"),
                "data".to_owned() => PathBuf::from("/dev/disk/by-bus/foobar"),
            },
            servicing_type: ServicingType::NoActiveServicing,
            ..Default::default()
        };

        assert_eq!(
            ctx.get_block_device_path(&"os".to_owned()).unwrap(),
            PathBuf::from("/dev/disk/by-bus/foobar")
        );
        assert_eq!(
            ctx.get_block_device_path(&"efi".to_owned()).unwrap(),
            PathBuf::from("/dev/disk/by-partlabel/osp1")
        );
        assert_eq!(
            ctx.get_block_device_path(&"root".to_owned()).unwrap(),
            PathBuf::from("/dev/disk/by-partlabel/osp2")
        );
        assert_eq!(ctx.get_block_device_path(&"foobar".to_owned()), None);
        assert_eq!(
            ctx.get_block_device_path(&"data".to_owned()).unwrap(),
            PathBuf::from("/dev/disk/by-bus/foobar")
        );

        // Now, set ab_active_volume to VolumeA.
        ctx.ab_active_volume = Some(AbVolumeSelection::VolumeA);
        assert_eq!(ctx.get_block_device_path(&"osab".to_owned()), None);
        assert_eq!(ctx.get_ab_volume_block_device_id(&"osab".to_owned()), None);

        // Now, set servicing type to AbUpdate.
        ctx.servicing_type = ServicingType::AbUpdate;
        assert_eq!(
            ctx.get_block_device_path(&"osab".to_owned()).unwrap(),
            PathBuf::from("/dev/disk/by-partlabel/osp3")
        );
        assert_eq!(
            ctx.get_ab_volume_block_device_id(&"osab".to_owned()),
            Some(&"rootb".to_owned())
        );

        // When active volume is VolumeB, should return VolumeA
        ctx.ab_active_volume = Some(AbVolumeSelection::VolumeB);
        assert_eq!(
            ctx.get_block_device_path(&"osab".to_owned()).unwrap(),
            PathBuf::from("/dev/disk/by-partlabel/osp2")
        );
        assert_eq!(
            ctx.get_ab_volume_block_device_id(&"osab".to_owned()),
            Some(&"root".to_owned())
        );

        // If target block device id does not exist, should return None.
        assert_eq!(
            ctx.get_ab_volume_block_device_id(&"non-existent".to_owned()),
            None
        );
    }

    /// Validates that get_ab_volume_pair() correctly returns the A/B volume pair.
    #[test]
    fn test_get_ab_volume_pair() {
        let mut ctx = EngineContext {
            ab_active_volume: Some(AbVolumeSelection::VolumeA),
            spec: HostConfiguration {
                ..Default::default()
            },
            ..Default::default()
        };

        // Test case #1: If there is no A/B update configuration provided, returns an error.
        assert_eq!(
            ctx.get_ab_volume_pair(&"root".to_owned())
                .unwrap_err()
                .root_cause()
                .to_string(),
            "No A/B update configuration found"
        );

        ctx.spec.storage.ab_update = Some(AbUpdate {
            volume_pairs: vec![AbVolumePair {
                id: "root".to_string(),
                volume_a_id: "root-a".to_string(),
                volume_b_id: "root-b".to_string(),
            }],
        });

        // Test case #2: If an A/B volume pair with the given ID does not exist, returns an error.
        assert_eq!(
            ctx.get_ab_volume_pair(&"non-existent".to_owned())
                .unwrap_err()
                .root_cause()
                .to_string(),
            "No volume pair for block device ID 'non-existent' found"
        );

        // Test case #3.1: If there are no block devices defined, returns an error.
        assert_eq!(
            ctx.get_ab_volume_pair(&"root".to_owned())
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Failed to get block device path for volume A with ID 'root-a'"
        );

        ctx.partition_paths.insert(
            "root-a".to_string(),
            PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}1")),
        );

        // Test case #3.2: If there are no block devices defined, returns an error.
        assert_eq!(
            ctx.get_ab_volume_pair(&"root".to_owned())
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Failed to get block device path for volume B with ID 'root-b'"
        );

        ctx.partition_paths.insert(
            "root-b".to_string(),
            PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2")),
        );

        // Test case #4: When information is complete, returns the volume pair paths.
        assert_eq!(
            ctx.get_ab_volume_pair(&"root".to_string()).unwrap(),
            AbVolumePairInfo {
                volume_a_path: PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}1")),
                volume_b_path: PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2")),
                volume_a_id: "root-a".to_string(),
                volume_b_id: "root-b".to_string(),
            }
        );
    }

    #[test]
    fn test_get_verity_config() {
        let mut ctx = EngineContext {
            spec: HostConfiguration {
                storage: config::Storage {
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };

        // Test case #0: If there is no internal verity device configuration, returns an error.
        assert_eq!(
            ctx.get_verity_config(&"root".to_owned())
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Failed to find configuration for verity device 'root'"
        );

        // Test case #1. Add a verity device config and ensure it is returned.
        ctx.spec.storage.verity = vec![VerityDevice {
            id: "root".to_string(),
            name: "root".to_string(),
            data_device_id: "root-data".to_string(),
            hash_device_id: "root-hash".to_string(),
            ..Default::default()
        }];

        assert_eq!(
            ctx.get_verity_config(&"root".to_owned()).unwrap(),
            VerityDevice {
                id: "root".to_string(),
                name: "root".to_string(),
                data_device_id: "root-data".to_string(),
                hash_device_id: "root-hash".to_string(),
                ..Default::default()
            }
        );

        // Test case #2: Requesting config for a non-existent device should return an error.
        assert_eq!(
            ctx.get_verity_config(&"non-existent".to_owned())
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Failed to find configuration for verity device 'non-existent'"
        );
    }

    #[test]
    fn test_filesystem_block_device_size() {
        let ctx = EngineContext::default().with_spec(HostConfiguration {
            storage: Storage {
                disks: vec![Disk {
                    id: "disk1".to_owned(),
                    device: PathBuf::from("/dev/sda"),
                    partitions: vec![Partition {
                        id: "part1".to_owned(),
                        size: 4096.into(),
                        partition_type: PartitionType::Root,
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

        assert_eq!(ctx.filesystem_block_device_size("/data"), Some(4096));

        assert_eq!(ctx.filesystem_block_device_size("/data/subdir"), Some(4096));

        assert_eq!(ctx.filesystem_block_device_size("/nonexistent"), None);
    }

    #[test]
    fn test_get_first_backing_partition() {
        let ctx = EngineContext {
            spec: HostConfiguration {
                storage: Storage {
                    disks: vec![Disk {
                        id: "os".to_owned(),
                        partitions: vec![
                            Partition {
                                id: "esp".to_owned(),
                                partition_type: PartitionType::Esp,
                                size: PartitionSize::from_str("1G").unwrap(),
                            },
                            Partition {
                                id: "root".to_owned(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::from_str("8G").unwrap(),
                            },
                            Partition {
                                id: "rootb".to_owned(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::from_str("8G").unwrap(),
                            },
                        ],
                        ..Default::default()
                    }],
                    raid: Raid {
                        software: vec![SoftwareRaidArray {
                            id: "root-raid1".to_owned(),
                            devices: vec!["root".to_string(), "rootb".to_string()],
                            name: "raid1".to_string(),
                            level: RaidLevel::Raid1,
                        }],
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };

        assert_eq!(
            ctx.get_first_backing_partition(&"esp".to_owned()).unwrap(),
            &ctx.spec.storage.disks[0].partitions[0]
        );
        assert_eq!(
            ctx.get_first_backing_partition(&"root".to_owned()).unwrap(),
            &ctx.spec.storage.disks[0].partitions[1]
        );
        assert_eq!(
            ctx.get_first_backing_partition(&"rootb".to_owned())
                .unwrap(),
            &ctx.spec.storage.disks[0].partitions[2]
        );
        assert_eq!(
            ctx.get_first_backing_partition(&"root-raid1".to_owned())
                .unwrap(),
            &ctx.spec.storage.disks[0].partitions[1]
        );
        ctx.get_first_backing_partition(&"os".to_owned())
            .unwrap_err();
        ctx.get_first_backing_partition(&"non-existant".to_owned())
            .unwrap_err();
    }
}
