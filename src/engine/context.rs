use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
};

use trident_api::{
    config::HostConfiguration,
    constants::ROOT_MOUNT_POINT_PATH,
    status::{AbVolumeSelection, ServicingType},
    storage_graph::graph::StorageGraph,
    BlockDeviceId,
};

use crate::osimage::OsImage;

#[cfg_attr(any(test, feature = "functional-test"), derive(Clone, Default))]
pub struct EngineContext {
    pub spec: HostConfiguration,

    pub spec_old: HostConfiguration,

    /// Type of servicing that Trident is executing on the host.
    pub servicing_type: ServicingType,

    /// The path associated with each block device in the Host Configuration.
    pub block_device_paths: BTreeMap<BlockDeviceId, PathBuf>,

    /// A/B update status.
    pub ab_active_volume: Option<AbVolumeSelection>,

    /// Stores the Disks UUID to ID mapping of the host.
    pub disks_by_uuid: HashMap<uuid::Uuid, BlockDeviceId>,

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
    #[allow(dead_code)]
    pub os_image: Option<OsImage>,

    /// The storage graph representing the storage configuration of the host.
    #[allow(dead_code)]
    pub storage_graph: StorageGraph,
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

    /// Returns a reference to the Partition object within an AB volume pair that corresponds to the
    /// update partition, or the one to be updated.
    #[cfg(feature = "sysupdate")]
    pub fn get_ab_update_volume_partition(
        &self,
        block_device_id: &BlockDeviceId,
    ) -> Option<&trident_api::config::Partition> {
        if let Some(ab_update) = self.spec.storage.ab_update.as_ref() {
            let ab_volume = ab_update
                .volume_pairs
                .iter()
                .find(|v| &v.id == block_device_id);
            if let Some(v) = ab_volume {
                return self
                    .get_ab_update_volume()
                    .and_then(|selection| match selection {
                        AbVolumeSelection::VolumeA => {
                            self.spec.storage.get_partition(&v.volume_a_id)
                        }
                        AbVolumeSelection::VolumeB => {
                            self.spec.storage.get_partition(&v.volume_b_id)
                        }
                    });
            }
        }

        None
    }

    /// Using the / mount point, figure out what should be used as a root block device.
    pub(super) fn get_root_block_device_path(&self) -> Option<PathBuf> {
        self.spec
            .storage
            .path_to_mount_point(Path::new(ROOT_MOUNT_POINT_PATH))
            .and_then(|m| self.get_block_device_path(&m.target_id))
    }

    /// Returns the path of the block device with id `block_device_id`.
    ///
    /// If the volume is part of an A/B Volume Pair this returns the update volume (i.e. the one that
    /// isn't active).
    pub(super) fn get_block_device_path(&self, block_device_id: &BlockDeviceId) -> Option<PathBuf> {
        if let Some(partition_path) = self.block_device_paths.get(block_device_id) {
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
            .internal_verity
            .iter()
            .find(|v| &v.id == block_device_id)
        {
            return Some(verity.device_path());
        }

        self.get_ab_volume_block_device_id(block_device_id)
            .and_then(|child_block_device_id| self.get_block_device_path(child_block_device_id))
    }

    /// Returns the block device id for the update volume from the given A/B Volume Pair.
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
}

#[cfg(test)]
mod tests {
    use super::*;

    use maplit::btreemap;

    use trident_api::config::{
        self, AbUpdate, AbVolumePair, Disk, FileSystemType, Partition, PartitionType,
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
                    internal_mount_points: vec![
                        config::InternalMountPoint {
                            target_id: "boot".to_owned(),
                            filesystem: FileSystemType::Vfat,
                            options: vec![],
                            path: PathBuf::from("/boot"),
                        },
                        config::InternalMountPoint {
                            target_id: "root".to_owned(),
                            filesystem: FileSystemType::Ext4,
                            options: vec![],
                            path: PathBuf::from(ROOT_MOUNT_POINT_PATH),
                        },
                    ],
                    ..Default::default()
                },
                ..Default::default()
            },
            block_device_paths: btreemap! {
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

    /// Validates that the `get_block_device_for_update` function works as expected for
    /// disks, partitions and ab volumes.
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
            block_device_paths: btreemap! {
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
}
