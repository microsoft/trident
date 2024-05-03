use std::{collections::BTreeMap, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    config::{HostConfiguration, Partition},
    BlockDeviceId,
};

/// HostStatus is the status of a host. Reflects the current state of the host and any encountered
/// errors.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostStatus {
    pub spec: HostConfiguration,

    /// Type of servicing that Trident is executing on the host.
    pub servicing_type: Option<ServicingType>,

    /// Current state of the servicing that Trident is executing on the host.
    pub servicing_state: ServicingState,

    #[serde(default)]
    pub trident: Trident,

    #[serde(default)]
    pub storage: Storage,

    /// BootNext variable of efibootmgr.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boot_next: Option<String>,
}

/// ServicingType is the type of servicing that the Trident agent is executing on the host. Through
/// ServicingType, Trident communicates what servicing operations are in progress.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum ServicingType {
    /// Update that can be applied without pausing the workload.
    HotPatch = 0,
    /// Update that requires pausing the workload.
    NormalUpdate = 1,
    /// Update that requires rebooting the host.
    UpdateAndReboot = 2,
    /// Update that requires switching to a different root partition and rebooting.
    AbUpdate = 3,
    /// Clean install of the runtime OS image when the host is booted from the provisioning OS.
    CleanInstall = 4,
    // Update that cannot be applied given the current state of the system.
    Incompatible = 5,
}

/// ServicingState describes the progress of the servicing that the Trident agent is executing on
/// the host. The host will transition through a different sequence of servicing states, depending
/// on the servicing type.
#[derive(Serialize, Deserialize, Copy, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum ServicingState {
    /// The host is running from the provisioning OS and has not yet been provisioned by Trident.
    #[default]
    NotProvisioned,
    /// Trident is now staging a new deployment, for the current servicing.
    StagingDeployment,
    /// Deployment has been staged, i.e., the updated runtime OS image has been deployed to block
    /// devices.
    DeploymentStaged,
    /// Trident is now finalizing the new deployment.
    FinalizingDeployment,
    /// Deployment has been finalized, i.e., UEFI variables have been set, so that firmware boots
    /// from the updated runtime OS image after reboot.
    DeploymentFinalized,
    /// Servicing of type CleanInstall has failed.
    CleanInstallFailed,
    /// Servicing of type AbUpdate has failed.
    AbUpdateFailed,
    /// Servicing has been completed, and the host succesfully booted from the updated runtime OS
    /// image. Trident is ready to begin a new servicing.
    Provisioned,
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
    /// Returns the update volume selection for all A/B volume pairs. The update volume is the one that
    /// is meant to be updated, based on the ongoing servicing type and state.
    pub fn get_ab_update_volume(&self) -> Option<AbVolumeSelection> {
        // If there is no A/B update configuration, return None
        self.spec.storage.ab_update.as_ref()?;

        match self.servicing_state {
            // If host is in NotProvisioned, CleanInstallFailed, Provisioned, or AbUpdateFailed,
            // update volume is None, since Trident is not executing any servicing
            ServicingState::NotProvisioned
            | ServicingState::CleanInstallFailed
            | ServicingState::Provisioned
            | ServicingState::AbUpdateFailed => None,
            // If host is in any different servicing state, determine based on servicing type
            ServicingState::StagingDeployment
            | ServicingState::DeploymentStaged
            | ServicingState::FinalizingDeployment
            | ServicingState::DeploymentFinalized => {
                match self.servicing_type {
                    Some(ServicingType::HotPatch)
                    | Some(ServicingType::NormalUpdate)
                    | Some(ServicingType::UpdateAndReboot) => self.storage.ab_active_volume,
                    // If host executing A/B update, update volume is the opposite of active volume
                    // as specified in the storage status
                    Some(ServicingType::AbUpdate) => {
                        if self.storage.ab_active_volume == Some(AbVolumeSelection::VolumeA) {
                            Some(AbVolumeSelection::VolumeB)
                        } else {
                            Some(AbVolumeSelection::VolumeA)
                        }
                    }
                    // If host is executing a clean install, update volume is always A
                    Some(ServicingType::CleanInstall) => Some(AbVolumeSelection::VolumeA),
                    Some(ServicingType::Incompatible) | None => None,
                }
            }
        }
    }

    /// Returns the active volume selection for all A/B volume pairs. The active volume is the one that
    /// the host is currently running from.
    pub fn get_ab_active_volume(&self) -> Option<AbVolumeSelection> {
        // If there is no A/B update configuration, return None
        self.spec.storage.ab_update.as_ref()?;

        match self.servicing_state {
            // If host is in NotProvisioned or CleanInstallFailed, there is no active volume, as
            // we're still booted from the provisioning OS
            ServicingState::NotProvisioned | ServicingState::CleanInstallFailed => None,
            // If host is in Provisioned OR AbUpdateFailed, active volume is the current one
            ServicingState::Provisioned | ServicingState::AbUpdateFailed => {
                self.storage.ab_active_volume
            }
            ServicingState::StagingDeployment
            | ServicingState::DeploymentStaged
            | ServicingState::FinalizingDeployment
            | ServicingState::DeploymentFinalized => {
                match self.servicing_type {
                    // If host is executing a deployment of any type, active volume is in host status
                    Some(ServicingType::HotPatch)
                    | Some(ServicingType::NormalUpdate)
                    | Some(ServicingType::UpdateAndReboot)
                    | Some(ServicingType::AbUpdate) => self.storage.ab_active_volume,
                    // If host is executing a clean install, there is no active volume yet
                    Some(ServicingType::CleanInstall)
                    | Some(ServicingType::Incompatible)
                    | None => None,
                }
            }
        }
    }

    /// Returns a reference to the Partition object within an AB volume pair that corresponds to the
    /// update partition, or the one to be updated.
    pub fn get_ab_update_volume_partition(
        &self,
        block_device_id: &BlockDeviceId,
    ) -> Option<&Partition> {
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
}

#[cfg(test)]
mod tests {
    use maplit::btreemap;

    use crate::config::{self, AbUpdate, AbVolumePair, Disk, PartitionType};

    use super::*;

    /// Validates that get_ab_update_volume_partition correctly returns the id of
    /// the active partition inside of an ab-volume pair.
    #[test]
    fn test_get_ab_update_volume_partition() {
        // Setting up the sample host_status
        let mut host_status = HostStatus {
            servicing_state: ServicingState::NotProvisioned,
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
        host_status.servicing_type = Some(ServicingType::AbUpdate);
        host_status.servicing_state = ServicingState::StagingDeployment;
        host_status.storage.ab_active_volume = Some(AbVolumeSelection::VolumeA);

        // Declare a new Partition object corresponding to the update partition root-b
        let partition_root_b = Partition {
            id: "root-b".to_owned(),
            size: config::PartitionSize::Fixed(10000),
            partition_type: PartitionType::Root,
        };

        assert_eq!(
            host_status.get_ab_update_volume_partition(&"root".to_owned()),
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
            host_status.get_ab_update_volume_partition(&"root".to_owned()),
            Some(&partition_root_a)
        );

        // 3. Test with an ID that doesn't match any volume pair
        assert_eq!(
            host_status.get_ab_update_volume_partition(&"nonexistent".to_owned()),
            None
        );
    }
}
