use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    config::{HostConfiguration, PartitionType, RaidLevel},
    BlockDeviceId,
};

/// HostStatus is the status of a host. Reflects the current provisioning state
/// of the host and any encountered errors.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostStatus {
    pub spec: HostConfiguration,

    pub reconcile_state: ReconcileState,

    #[serde(default)]
    pub trident: Trident,

    #[serde(default)]
    pub storage: Storage,

    /// BootNext variable of efibootmgr.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boot_next: Option<String>,
}

/// ReconcileState is the state of the host's reconciliation process. Through
/// the ReconcileState, the Trident agent communicates what operations are in progress.
#[derive(Serialize, Deserialize, Copy, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum ReconcileState {
    /// A clean install is in progress.
    CleanInstall,
    /// An update is in progress.
    UpdateInProgress(UpdateKind),
    /// The system is running normally.
    #[default]
    Ready,
}

/// UpdateKind is the kind of update that is in progress.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum UpdateKind {
    /// Update that can be applied without pausing the workload.
    HotPatch = 0,
    /// Update that requires pausing the workload.
    NormalUpdate = 1,
    /// Update that requires rebooting the host.
    UpdateAndReboot = 2,
    /// Update that requires switching to a different root partition and rebooting.
    AbUpdate = 3,
    /// Update that cannot be applied given the current state of the system.
    Incompatible = 4,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Trident {
    pub datastore_path: Option<PathBuf>,
}

/// Storage status of a host.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Storage {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub disks: BTreeMap<BlockDeviceId, Disk>,

    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub raid_arrays: BTreeMap<BlockDeviceId, RaidArray>,

    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub encrypted_volumes: BTreeMap<BlockDeviceId, EncryptedVolume>,

    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub verity_devices: BTreeMap<BlockDeviceId, VerityDevice>,

    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub mount_points: BTreeMap<PathBuf, MountPoint>,

    /// A/B update status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ab_update: Option<AbUpdate>,

    /// Path to the root block device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_device_path: Option<PathBuf>,
}

/// Per disk status.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Disk {
    pub uuid: Uuid,
    pub path: PathBuf,
    pub capacity: u64,
    pub partitions: Vec<Partition>,
    pub contents: BlockDeviceContents,
}

impl Disk {
    pub fn to_block_device(&self) -> BlockDeviceInfo {
        BlockDeviceInfo::new(self.path.clone(), self.capacity, self.contents.clone())
    }
}

/// Per partition status.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Partition {
    pub id: BlockDeviceId,

    pub path: PathBuf,
    pub start: u64,
    pub end: u64,
    #[serde(rename = "type")]
    pub ty: PartitionType,
    pub contents: BlockDeviceContents,
    pub uuid: Uuid,
}

/// Status of contents of a block device.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum BlockDeviceContents {
    /// Default state when no specific initialization of the block device has been performed.
    #[default]
    Unknown,

    /// Block device has been zeroed out.
    Zeroed,

    /// Block device has been initialized using an image.
    Image {
        sha256: String,
        length: u64,
        url: String,
    },

    /// Block device has been initialized in some other way besides an image or zeroing.
    Initialized,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EncryptedVolume {
    /// The name of the device created under `/dev/mapper` when opening
    /// the volume.
    pub device_name: String,

    /// The path of the disk partition or software raid array encrypted.
    pub target_path: PathBuf,

    /// The inherited partition type of the encrypted volume.
    /// This is the partition type of the underlying partition or raid array.
    pub partition_type: PartitionType,

    /// The size of the encrypted volume.
    pub size: u64,

    /// The contents of the encrypted volume.
    pub contents: BlockDeviceContents,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct VerityDevice {
    /// The name of the device created under `/dev/mapper` when opening
    /// the volume.
    pub device_name: String,

    /// Root hash of the verity device.
    pub root_hash: String,
}

// Status of a raid array.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RaidArray {
    /// Unique identifier of the raid array.
    pub name: String,

    /// List of paths of devices (partitions) that take part in the RAID.
    pub device_paths: Vec<PathBuf>,

    /// The inherited partition type of the RAID array. This is the
    /// partition type of the underlying devices (partitions).
    pub partition_type: PartitionType,

    /// RAID level.
    pub level: RaidLevel,

    /// RAID status (created, ready, failed).
    pub status: RaidArrayStatus,

    /// RAID array size.
    pub array_size: u64,

    /// RAID array type.
    #[serde(rename = "type")]
    pub ty: RaidType,

    /// Path to the raid array. For example, /dev/md/{name}
    pub path: PathBuf,

    /// UUID of the RAID device
    pub uuid: Uuid,

    /// RAID array contents.
    pub contents: BlockDeviceContents,
}

/// Type of RAID array (software, hardware). Only software for now.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum RaidType {
    Software,
}

/// Status of a RAID array in Trident host status.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum RaidArrayStatus {
    Created,
    Ready,
    Failed,
}
/// Mount point status.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MountPoint {
    pub target_id: BlockDeviceId,
    pub filesystem: String,
    pub options: Vec<String>,
}

/// A/B update status. Carries information about the A/B update volume pairs and
/// the currently active volume. Note that all pairs will have at any point in
/// time the same volume (A or B) active. The volume to update is determined by
/// the ReconcileState and active_volume.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AbUpdate {
    /// Map from AB volume pair block device id to the AB volume pair.
    pub volume_pairs: BTreeMap<BlockDeviceId, AbVolumePair>,
    /// Determines which set of volumes are currently active.
    pub active_volume: Option<AbVolumeSelection>,
}

/// A/B volume selection. Determines which set of volumes are currently
/// active/used by the OS.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum AbVolumeSelection {
    VolumeA,
    VolumeB,
}

/// Per A/B update volume pair status.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AbVolumePair {
    pub volume_a_id: BlockDeviceId,
    pub volume_b_id: BlockDeviceId,
}

/// Block device information. Carries information about the block device path
/// and size, used for storage. Abstracts the difference between specific block
/// device types.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BlockDeviceInfo {
    pub path: PathBuf,
    pub size: u64,
    pub contents: BlockDeviceContents,
}

impl HostStatus {
    /// Returns the volume selection for all AB Volume Pairs.
    ///
    /// This is used to determine which volumes are currently active and which
    /// are meant for updating. In addition, if active is true and an A/B update
    /// is in progress, the active volume selection will be returned. If active
    /// is false, the volume selection corresponding to the volumes to be
    /// updated will be returned.
    pub fn get_ab_update_volume(&self, active: bool) -> Option<AbVolumeSelection> {
        let active_volume = self.storage.ab_update.as_ref()?.active_volume;
        match self.reconcile_state {
            ReconcileState::UpdateInProgress(UpdateKind::HotPatch)
            | ReconcileState::UpdateInProgress(UpdateKind::NormalUpdate)
            | ReconcileState::UpdateInProgress(UpdateKind::UpdateAndReboot) => active_volume,
            ReconcileState::UpdateInProgress(UpdateKind::AbUpdate) => {
                if active {
                    active_volume
                } else {
                    Some(if active_volume == Some(AbVolumeSelection::VolumeA) {
                        AbVolumeSelection::VolumeB
                    } else {
                        AbVolumeSelection::VolumeA
                    })
                }
            }
            ReconcileState::UpdateInProgress(UpdateKind::Incompatible) => None,
            ReconcileState::Ready => {
                if active {
                    active_volume
                } else {
                    None
                }
            }
            ReconcileState::CleanInstall => Some(AbVolumeSelection::VolumeA),
        }
    }

    /// Returns a reference to the Partition object within an AB volume pair that corresponds to the
    /// inactive partition, or the one to be updated.
    pub fn get_ab_volume_partition(&self, block_device_id: &BlockDeviceId) -> Option<&Partition> {
        if let Some(ab_update) = self.storage.ab_update.as_ref() {
            let ab_volume = ab_update
                .volume_pairs
                .iter()
                .find(|v| v.0 == block_device_id);
            if let Some(v) = ab_volume {
                return self
                    .get_ab_update_volume(false)
                    .and_then(|selection| match selection {
                        AbVolumeSelection::VolumeA => {
                            self.storage.get_partition_ref(&v.1.volume_a_id)
                        }
                        AbVolumeSelection::VolumeB => {
                            self.storage.get_partition_ref(&v.1.volume_b_id)
                        }
                    });
            }
        }

        None
    }
}

impl Storage {
    /// Returns a reference to Partition corresponding to block_device_id.
    pub fn get_partition_ref(&self, block_device_id: &BlockDeviceId) -> Option<&Partition> {
        self.disks
            .iter()
            .flat_map(|(_block_device_id, disk)| &disk.partitions)
            .find(|p| p.id == *block_device_id)
    }

    /// Returns the mount point and relative path for a given path.
    ///
    /// The mount point is the closest parent directory of the path that is a
    /// mount point. The relative path is the path relative to the mount point.
    pub fn get_mount_point_and_relative_path<'a>(
        &'a self,
        path: &'a Path,
    ) -> Option<(&MountPoint, &Path)> {
        self.mount_points
            .iter()
            .filter(|(k, _)| path.starts_with(k))
            .max_by_key(|(k, _)| k.components().count())
            .and_then(|(k, v)| Some((v, path.strip_prefix(k).ok()?)))
    }

    /// Thie function returns the filesystem of the mount point targetting
    /// the given block device id or, if part of an A/B update volume
    /// pair, the mount point targetting the A/B update volume pair it is
    /// apart of.
    ///
    /// If no such mount point exists, None is returned.
    ///
    /// Block device IDs that are part of RAID arrays or verity devices
    /// are not supported. In such cases, None is returned.
    pub fn get_filesystem(&self, bdid: &BlockDeviceId) -> Option<&String> {
        // Recursive case: Check if the block device is part of an A/B
        // update volume pair, and if so, return the filesystem of A/B
        // update volume part itself.
        if let Some(ab_update) = &self.ab_update {
            if let Some(pair) = ab_update
                .volume_pairs
                .iter()
                .find(|(_, p)| p.volume_a_id == *bdid || p.volume_b_id == *bdid)
            {
                return self.get_filesystem(pair.0);
            }
        }

        // Base case: Check if the block device is directly mounted, and
        // if so, return the filesystem of the mount point.
        self.mount_points.iter().find_map(|(_, mp)| {
            if mp.target_id == *bdid {
                Some(&mp.filesystem)
            } else {
                None
            }
        })
    }

    /// Find the mount point that is holding the given path. This is useful to find
    /// the volume on which the given absolute path is located. This version uses HS
    /// to find the information and is preferred as it refers to the status of the system.
    pub fn path_to_mount_point<'a>(&'a self, path: &Path) -> Option<&'a MountPoint> {
        self.mount_points
            .iter()
            .filter(|(mp_path, _)| path.starts_with(mp_path))
            .max_by_key(|(mp_path, _)| mp_path.as_os_str().len())
            .map(|(_, mp)| mp)
    }
}

#[cfg(test)]
mod tests {
    use maplit::btreemap;

    use crate::constants::ROOT_MOUNT_POINT_PATH;

    use super::*;

    /// Validates logic for querying disks and partitions.
    #[test]
    fn test_get_partition_ref() {
        let host_status = HostStatus {
            storage: Storage {
                disks: btreemap! {
                    "os".into() => Disk {
                        path: PathBuf::from("/dev/disk/by-bus/foobar"),
                        uuid: Uuid::nil(),
                        capacity: 0,
                        contents: BlockDeviceContents::Unknown,
                        partitions: vec![
                            Partition {
                                id: "efi".to_string(),
                                path: PathBuf::from("/dev/disk/by-partlabel/osp1"),
                                contents: BlockDeviceContents::Unknown,
                                start: 0,
                                end: 0,
                                ty: PartitionType::Esp,
                                uuid: Uuid::nil(),
                            },
                            Partition {
                                id: "root".to_string(),
                                path: PathBuf::from("/dev/disk/by-partlabel/osp2"),
                                contents: BlockDeviceContents::Unknown,
                                start: 100,
                                end: 1000,
                                ty: PartitionType::Root,
                                uuid: Uuid::nil(),
                            },
                            Partition {
                                id: "rootb".to_string(),
                                path: PathBuf::from("/dev/disk/by-partlabel/osp3"),
                                contents: BlockDeviceContents::Unknown,
                                start: 1000,
                                end: 10000,
                                ty: PartitionType::Root,
                                uuid: Uuid::nil(),
                            },
                        ],
                    },
                },
                ..Default::default()
            },
            reconcile_state: ReconcileState::CleanInstall,
            ..Default::default()
        };

        // New assertions for get_partition_ref
        assert_eq!(
            host_status.storage.get_partition_ref(&"os".to_owned()),
            None
        );
        assert_eq!(
            host_status
                .storage
                .get_partition_ref(&"efi".to_owned())
                .map(|p| &p.path),
            Some(&PathBuf::from("/dev/disk/by-partlabel/osp1"))
        );
    }

    /// Validates that get_ab_volume_partition() correctly returns the id of
    /// the active partition inside of an ab-volume pair.
    #[test]
    fn test_get_ab_volume_partition() {
        // Setting up the sample host_status
        let mut host_status = HostStatus {
            reconcile_state: ReconcileState::CleanInstall,
            storage: Storage {
                disks: btreemap! {
                    "os".into() => Disk {
                        path: PathBuf::from("/dev/disk/by-bus/foobar"),
                        uuid: Uuid::nil(),
                        capacity: 0,
                        contents: BlockDeviceContents::Unknown,
                        partitions: vec![
                            Partition {
                                id: "efi".to_string(),
                                path: PathBuf::from("/dev/disk/by-partlabel/osp1"),
                                contents: BlockDeviceContents::Unknown,
                                start: 0,
                                end: 0,
                                ty: PartitionType::Esp,
                                uuid: Uuid::nil(),
                            },
                            Partition {
                                id: "root-a".to_string(),
                                path: PathBuf::from("/dev/disk/by-partlabel/osp2"),
                                contents: BlockDeviceContents::Unknown,
                                start: 100,
                                end: 1000,
                                ty: PartitionType::Root,
                                uuid: Uuid::nil(),
                            },
                            Partition {
                                id: "root-b".to_string(),
                                path: PathBuf::from("/dev/disk/by-partlabel/osp3"),
                                contents: BlockDeviceContents::Unknown,
                                start: 1000,
                                end: 10000,
                                ty: PartitionType::Root,
                                uuid: Uuid::nil(),
                            },
                        ],
                    },
                    "data".into() => Disk {
                        path: PathBuf::from("/dev/disk/by-bus/foobar"),
                        uuid: Uuid::nil(),
                        capacity: 1000,
                        contents: BlockDeviceContents::Unknown,
                        partitions: vec![],
                    },
                },
                ab_update: Some(AbUpdate {
                    volume_pairs: btreemap! {
                        "root".to_string() => AbVolumePair {
                            volume_a_id: "root-a".to_string(),
                            volume_b_id: "root-b".to_string(),
                        },
                    },
                    active_volume: Some(AbVolumeSelection::VolumeA),
                }),
                ..Default::default()
            },
            ..Default::default()
        };

        // 1. Test when the active volume is VolumeA
        host_status.reconcile_state = ReconcileState::UpdateInProgress(UpdateKind::AbUpdate);
        host_status
            .storage
            .ab_update
            .as_mut()
            .unwrap()
            .active_volume = Some(AbVolumeSelection::VolumeA);

        // Declare a new Partition object corresponding to the inactive
        // partition root-b
        let partition_root_b = Partition {
            id: "root-b".to_owned(),
            path: PathBuf::from("/dev/disk/by-partlabel/osp3"),
            contents: BlockDeviceContents::Unknown,
            start: 1000,
            end: 10000,
            ty: PartitionType::Root,
            uuid: uuid::Uuid::nil(),
        };

        assert_eq!(
            host_status.get_ab_volume_partition(&"root".to_owned()),
            Some(&partition_root_b)
        );

        // 2. Test when the active volume is VolumeB
        host_status
            .storage
            .ab_update
            .as_mut()
            .unwrap()
            .active_volume = Some(AbVolumeSelection::VolumeB);

        // Declare a new Partition object
        let partition_root_a = Partition {
            id: "root-a".to_owned(),
            path: PathBuf::from("/dev/disk/by-partlabel/osp2"),
            contents: BlockDeviceContents::Unknown,
            start: 100,
            end: 1000,
            ty: PartitionType::Root,
            uuid: uuid::Uuid::nil(),
        };

        assert_eq!(
            host_status.get_ab_volume_partition(&"root".to_owned()),
            Some(&partition_root_a)
        );

        // 3. Test with an ID that doesn't match any volume pair
        assert_eq!(
            host_status.get_ab_volume_partition(&"nonexistent".to_owned()),
            None
        );
    }

    #[test]
    fn test_get_mount_point_and_relative_path() {
        let host_status = HostStatus {
            storage: Storage {
                mount_points: btreemap! {
                    PathBuf::from("/") => MountPoint {
                        target_id: "root".into(),
                        filesystem: "ext4".into(),
                        options: vec![],
                    },
                    PathBuf::from("/boot") => MountPoint {
                        target_id: "boot".into(),
                        filesystem: "ext4".into(),
                        options: vec![],
                    },
                    PathBuf::from("/boot/efi") => MountPoint {
                        target_id: "efi".into(),
                        filesystem: "vfat".into(),
                        options: vec![],
                    },
                },
                ..Default::default()
            },
            ..Default::default()
        };

        assert_eq!(
            host_status
                .storage
                .get_mount_point_and_relative_path(&PathBuf::from("/")),
            Some((
                &MountPoint {
                    target_id: "root".into(),
                    filesystem: "ext4".into(),
                    options: vec![],
                },
                Path::new("")
            ))
        );

        assert_eq!(
            host_status
                .storage
                .get_mount_point_and_relative_path(&PathBuf::from("/boot/efi.cfg")),
            Some((
                &MountPoint {
                    target_id: "boot".into(),
                    filesystem: "ext4".into(),
                    options: vec![],
                },
                Path::new("efi.cfg")
            ))
        );

        assert_eq!(
            host_status
                .storage
                .get_mount_point_and_relative_path(&PathBuf::from("/boot/efi")),
            Some((
                &MountPoint {
                    target_id: "efi".into(),
                    filesystem: "vfat".into(),
                    options: vec![],
                },
                Path::new("")
            ))
        );

        assert_eq!(
            host_status
                .storage
                .get_mount_point_and_relative_path(&PathBuf::from("/boot/efi/")),
            Some((
                &MountPoint {
                    target_id: "efi".into(),
                    filesystem: "vfat".into(),
                    options: vec![],
                },
                Path::new("")
            ))
        );

        assert_eq!(
            host_status
                .storage
                .get_mount_point_and_relative_path(&PathBuf::from("/boot/efi/foobar")),
            Some((
                &MountPoint {
                    target_id: "efi".into(),
                    filesystem: "vfat".into(),
                    options: vec![],
                },
                Path::new("foobar")
            ))
        );

        assert_eq!(
            host_status
                .storage
                .get_mount_point_and_relative_path(&PathBuf::from("/boot/efi/foobar/")),
            Some((
                &MountPoint {
                    target_id: "efi".into(),
                    filesystem: "vfat".into(),
                    options: vec![],
                },
                Path::new("foobar")
            ))
        );
    }

    #[test]
    fn test_get_filesystem_single_mount_point_id_match_returns_filesystem() {
        let storage = Storage {
            disks: BTreeMap::new(),
            raid_arrays: BTreeMap::new(),
            encrypted_volumes: BTreeMap::new(),
            verity_devices: BTreeMap::new(),
            mount_points: btreemap! {
                PathBuf::from("/") => MountPoint {
                    target_id: "root".into(),
                    filesystem: "ext4".into(),
                    options: vec![],
                },
            },
            ab_update: Some(AbUpdate {
                volume_pairs: BTreeMap::new(),
                active_volume: None,
            }),
            root_device_path: None,
        };

        assert_eq!(storage.get_filesystem(&"root".into()).unwrap(), "ext4");
    }

    #[test]
    fn test_get_filesystem_three_mount_points_id_match_returns_filesystem() {
        let storage = Storage {
            disks: BTreeMap::new(),
            raid_arrays: BTreeMap::new(),
            encrypted_volumes: BTreeMap::new(),
            verity_devices: BTreeMap::new(),
            mount_points: btreemap! {
                PathBuf::from("/") => MountPoint {
                    target_id: "root".into(),
                    filesystem: "ext4".into(),
                    options: vec![],
                },
                PathBuf::from("/boot") => MountPoint {
                    target_id: "boot".into(),
                    filesystem: "ext4".into(),
                    options: vec![],
                },
                PathBuf::from("/boot/efi") => MountPoint {
                    target_id: "efi".into(),
                    filesystem: "vfat".into(),
                    options: vec![],
                },
            },
            ab_update: Some(AbUpdate {
                volume_pairs: BTreeMap::new(),
                active_volume: None,
            }),
            root_device_path: None,
        };

        assert_eq!(storage.get_filesystem(&"efi".into()).unwrap(), "vfat");
    }

    #[test]
    fn test_get_filesystem_match_root_returns_ext4() {
        let storage = Storage {
            disks: BTreeMap::new(),
            raid_arrays: BTreeMap::new(),
            encrypted_volumes: BTreeMap::new(),
            verity_devices: BTreeMap::new(),
            mount_points: btreemap! {
                PathBuf::from("/") => MountPoint {
                    target_id: "root".into(),
                    filesystem: "ext4".into(),
                    options: vec![],
                },
                PathBuf::from("/boot") => MountPoint {
                    target_id: "boot".into(),
                    filesystem: "ext4".into(),
                    options: vec![],
                },
                PathBuf::from("/boot/efi") => MountPoint {
                    target_id: "efi".into(),
                    filesystem: "vfat".into(),
                    options: vec![],
                },
            },
            ab_update: Some(AbUpdate {
                volume_pairs: BTreeMap::new(),
                active_volume: None,
            }),
            root_device_path: None,
        };

        assert_eq!(storage.get_filesystem(&"root".into()).unwrap(), "ext4");
    }

    #[test]
    fn test_get_filesystem_no_match_returns_none() {
        let storage = Storage {
            disks: BTreeMap::new(),
            raid_arrays: BTreeMap::new(),
            encrypted_volumes: BTreeMap::new(),
            verity_devices: BTreeMap::new(),
            mount_points: btreemap! {
                PathBuf::from("/") => MountPoint {
                    target_id: "root".into(),
                    filesystem: "ext4".into(),
                    options: vec![],
                },
                PathBuf::from("/boot") => MountPoint {
                    target_id: "boot".into(),
                    filesystem: "ext4".into(),
                    options: vec![],
                },
                PathBuf::from("/boot/efi") => MountPoint {
                    target_id: "efi".into(),
                    filesystem: "vfat".into(),
                    options: vec![],
                },
            },
            ab_update: Some(AbUpdate {
                volume_pairs: BTreeMap::new(),
                active_volume: None,
            }),
            root_device_path: None,
        };

        assert!(storage.get_filesystem(&"srv".into()).is_none());
    }

    #[test]
    fn test_get_filesystem_match_ab_root_by_pair_id_returns_ext4() {
        let storage = Storage {
            disks: BTreeMap::new(),
            raid_arrays: BTreeMap::new(),
            encrypted_volumes: BTreeMap::new(),
            verity_devices: BTreeMap::new(),
            mount_points: btreemap! {
                PathBuf::from("/") => MountPoint {
                    target_id: "root".into(),
                    filesystem: "ext4".into(),
                    options: vec![],
                },
                PathBuf::from("/boot") => MountPoint {
                    target_id: "boot".into(),
                    filesystem: "ext4".into(),
                    options: vec![],
                },
                PathBuf::from("/boot/efi") => MountPoint {
                    target_id: "efi".into(),
                    filesystem: "vfat".into(),
                    options: vec![],
                },
            },
            ab_update: Some(AbUpdate {
                volume_pairs: btreemap! {
                    "root".into() => AbVolumePair {
                        volume_a_id: "root-a".into(),
                        volume_b_id: "root-b".into(),
                    },
                },
                active_volume: None,
            }),
            root_device_path: None,
        };

        assert_eq!(storage.get_filesystem(&"root".into()).unwrap(), "ext4");
    }

    #[test]
    fn test_get_filesystem_match_ab_root_by_vol_a_id_returns_ext4() {
        let storage = Storage {
            disks: BTreeMap::new(),
            raid_arrays: BTreeMap::new(),
            encrypted_volumes: BTreeMap::new(),
            verity_devices: BTreeMap::new(),
            mount_points: btreemap! {
                PathBuf::from("/") => MountPoint {
                    target_id: "root".into(),
                    filesystem: "ext4".into(),
                    options: vec![],
                },
                PathBuf::from("/boot") => MountPoint {
                    target_id: "boot".into(),
                    filesystem: "ext4".into(),
                    options: vec![],
                },
                PathBuf::from("/boot/efi") => MountPoint {
                    target_id: "efi".into(),
                    filesystem: "vfat".into(),
                    options: vec![],
                },
            },
            ab_update: Some(AbUpdate {
                volume_pairs: btreemap! {
                    "root".into() => AbVolumePair {
                        volume_a_id: "root-a".into(),
                        volume_b_id: "root-b".into(),
                    },
                },
                active_volume: None,
            }),
            root_device_path: None,
        };

        assert_eq!(storage.get_filesystem(&"root-a".into()).unwrap(), "ext4");
    }

    #[test]
    fn test_path_to_mount_point() {
        let mut host_status = HostStatus {
            reconcile_state: ReconcileState::CleanInstall,
            ..Default::default()
        };
        let mount_point = MountPoint {
            target_id: "part1".to_owned(),
            filesystem: "ext4".to_owned(),
            options: vec![],
        };
        host_status.storage.mount_points.insert(
            PathBuf::from(ROOT_MOUNT_POINT_PATH).join("boot"),
            mount_point.clone(),
        );

        let mount_point = host_status
            .storage
            .path_to_mount_point(Path::new(ROOT_MOUNT_POINT_PATH).join("boot").as_path())
            .unwrap();
        assert_eq!(mount_point.target_id, "part1");

        // ensure to pick the longest prefix
        host_status.storage.mount_points.insert(
            PathBuf::from(ROOT_MOUNT_POINT_PATH),
            MountPoint {
                filesystem: "ext4".to_owned(),
                options: vec![],
                target_id: "part2".to_owned(),
            },
        );

        let mount_point = host_status
            .storage
            .path_to_mount_point(Path::new(ROOT_MOUNT_POINT_PATH).join("boot").as_path())
            .unwrap();
        assert_eq!(mount_point.target_id, "part1");

        // validate longer paths
        let mount_point = host_status
            .storage
            .path_to_mount_point(
                Path::new(ROOT_MOUNT_POINT_PATH)
                    .join("boot/foo/bar")
                    .as_path(),
            )
            .unwrap();
        assert_eq!(mount_point.target_id, "part1");

        let mount_point = host_status
            .storage
            .path_to_mount_point(Path::new(ROOT_MOUNT_POINT_PATH).join("foo/bar").as_path())
            .unwrap();
        assert_eq!(mount_point.target_id, "part2");

        // validate failure without any mount points
        host_status.storage.mount_points.clear();
        assert!(host_status
            .storage
            .path_to_mount_point(Path::new(ROOT_MOUNT_POINT_PATH).join("boot").as_path())
            .is_none());
    }
}
