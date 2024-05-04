use std::{collections::BTreeMap, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    config::{HostConfiguration, Partition},
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
    pub block_devices: BTreeMap<BlockDeviceId, BlockDeviceInfo>,

    /// A/B update status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ab_active_volume: Option<AbVolumeSelection>,

    /// Path to the root block device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_device_path: Option<PathBuf>,
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

/// A/B volume selection. Determines which set of volumes are currently
/// active/used by the OS.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum AbVolumeSelection {
    VolumeA,
    VolumeB,
}

/// Block device information. Carries information about the block device path
/// and size, used for storage. Abstracts the difference between specific block
/// device types.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
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
        self.spec.storage.ab_update.as_ref()?;

        match self.reconcile_state {
            ReconcileState::UpdateInProgress(UpdateKind::HotPatch)
            | ReconcileState::UpdateInProgress(UpdateKind::NormalUpdate)
            | ReconcileState::UpdateInProgress(UpdateKind::UpdateAndReboot) => {
                self.storage.ab_active_volume
            }
            ReconcileState::UpdateInProgress(UpdateKind::AbUpdate) => {
                if active {
                    self.storage.ab_active_volume
                } else {
                    Some(
                        if self.storage.ab_active_volume == Some(AbVolumeSelection::VolumeA) {
                            AbVolumeSelection::VolumeB
                        } else {
                            AbVolumeSelection::VolumeA
                        },
                    )
                }
            }
            ReconcileState::UpdateInProgress(UpdateKind::Incompatible) => None,
            ReconcileState::Ready => {
                if active {
                    self.storage.ab_active_volume
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
        if let Some(ab_update) = self.spec.storage.ab_update.as_ref() {
            let ab_volume = ab_update
                .volume_pairs
                .iter()
                .find(|v| &v.id == block_device_id);
            if let Some(v) = ab_volume {
                return self
                    .get_ab_update_volume(false)
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
}

#[cfg(test)]
mod tests {
    use maplit::btreemap;

    use crate::config::{self, AbUpdate, AbVolumePair, Disk, PartitionType};

    use super::*;

    /// Validates that get_ab_volume_partition() correctly returns the id of
    /// the active partition inside of an ab-volume pair.
    #[test]
    fn test_get_ab_volume_partition() {
        // Setting up the sample host_status
        let mut host_status = HostStatus {
            reconcile_state: ReconcileState::CleanInstall,
            spec: HostConfiguration {
                storage: config::Storage {
                    disks: vec![Disk {
                        id: "os".to_string(),
                        device: PathBuf::from("/dev/disk/by-bus/foobar"),
                        partitions: vec![
                            Partition {
                                id: "efi".to_string(),
                                partition_type: PartitionType::Esp,
                                size: config::PartitionSize::Fixed(100),
                            },
                            Partition {
                                id: "root-a".to_string(),
                                partition_type: PartitionType::Root,
                                size: config::PartitionSize::Fixed(1000),
                            },
                            Partition {
                                id: "root-b".to_string(),
                                partition_type: PartitionType::Root,
                                size: config::PartitionSize::Fixed(10000),
                            },
                        ],
                        ..Default::default()
                    }],
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
            },
            storage: Storage {
                block_devices: btreemap! {
                    "os".into() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-bus/foobar"),
                        size: 0,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "efi".into() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-partlabel/osp1"),
                        size: 100,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "root-a".into() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-partlabel/osp2"),
                        size: 1000,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "root-b".into() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-partlabel/osp3"),
                        size: 10000,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "data".into() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-bus/foobar"),
                        size: 1000,
                        contents: BlockDeviceContents::Unknown,
                    },
                },
                ab_active_volume: Some(AbVolumeSelection::VolumeA),
                ..Default::default()
            },
            ..Default::default()
        };

        // 1. Test when the active volume is VolumeA
        host_status.reconcile_state = ReconcileState::UpdateInProgress(UpdateKind::AbUpdate);
        host_status.storage.ab_active_volume = Some(AbVolumeSelection::VolumeA);

        // Declare a new Partition object corresponding to the inactive
        // partition root-b
        let partition_root_b = Partition {
            id: "root-b".to_owned(),
            size: config::PartitionSize::Fixed(10000),
            partition_type: PartitionType::Root,
        };

        assert_eq!(
            host_status.get_ab_volume_partition(&"root".to_owned()),
            Some(&partition_root_b)
        );

        // 2. Test when the active volume is VolumeB
        host_status.storage.ab_active_volume = Some(AbVolumeSelection::VolumeB);

        // Declare a new Partition object
        let partition_root_a = Partition {
            id: "root-a".to_owned(),
            size: config::PartitionSize::Fixed(1000),
            partition_type: PartitionType::Root,
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
}
