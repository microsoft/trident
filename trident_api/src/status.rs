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
    pub storage: Storage,

    /// BootNext variable of efibootmgr.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boot_next: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<serde_yaml::Value>,
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
    /// Trident is now staging a new servicing.
    Staging,
    /// Servicing has been staged, i.e., the updated runtime OS image has been deployed onto block
    /// devices.
    Staged,
    /// Trident is finalizing the ongoing servicing.
    Finalizing,
    /// Servicing has been finalized, i.e., UEFI variables have been set, so that firmware boots
    /// from the updated runtime OS image after reboot.
    Finalized,
    /// Servicing of type CleanInstall has failed.
    CleanInstallFailed,
    /// Servicing of type AbUpdate has failed.
    AbUpdateFailed,
    /// Servicing has been completed, and the host succesfully booted from the updated runtime OS
    /// image. Trident is ready to begin a new servicing.
    Provisioned,
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
    /// Returns the update volume selection for all A/B volume pairs. The update
    /// volume is the one that is meant to be updated, based on the ongoing
    /// servicing type and state.
    pub fn get_ab_update_volume(&self) -> Option<AbVolumeSelection> {
        match &self.servicing_state {
            // If host is in NotProvisioned, CleanInstallFailed, Provisioned, or AbUpdateFailed,
            // update volume is None, since Trident is not executing any servicing
            ServicingState::NotProvisioned
            | ServicingState::CleanInstallFailed
            | ServicingState::Provisioned
            | ServicingState::AbUpdateFailed => None,
            // If host is in any different servicing state, determine based on servicing type
            ServicingState::Staging
            | ServicingState::Staged
            | ServicingState::Finalizing
            | ServicingState::Finalized => {
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
                    // In host status, servicing type will never be set to Incompatible OR be None if
                    // servicing state is one of the above.
                    Some(ServicingType::Incompatible) | None => None,
                }
            }
        }
    }

    /// Returns the active volume selection for all A/B volume pairs. The active
    /// volume is the one that the host is currently running from.
    pub fn get_ab_active_volume(&self) -> Option<AbVolumeSelection> {
        match self.servicing_state {
            // If host is in NotProvisioned or CleanInstallFailed, there is no active volume, as
            // we're still booted from the provisioning OS
            ServicingState::NotProvisioned | ServicingState::CleanInstallFailed => None,
            // If host is in Provisioned OR AbUpdateFailed, active volume is the current one
            ServicingState::Provisioned | ServicingState::AbUpdateFailed => {
                self.storage.ab_active_volume
            }
            ServicingState::Staging
            | ServicingState::Staged
            | ServicingState::Finalizing
            | ServicingState::Finalized => {
                match self.servicing_type {
                    // If host is executing a deployment of any type, active volume is in host status.
                    Some(ServicingType::HotPatch)
                    | Some(ServicingType::NormalUpdate)
                    | Some(ServicingType::UpdateAndReboot)
                    | Some(ServicingType::AbUpdate) => self.storage.ab_active_volume,
                    // If host is executing a clean install, there is no active volume yet.
                    Some(ServicingType::CleanInstall) => None,
                    // In host status, servicing type will never be set to Incompatible OR be None if
                    // servicing state is one of the above.
                    Some(ServicingType::Incompatible) | None => unreachable!(),
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
    use crate::config::{self, AbUpdate};

    use super::*;

    /// Validates logic in get_ab_update_volume() function
    #[test]
    fn test_get_ab_update_volume() {
        let mut host_status = HostStatus {
            spec: HostConfiguration {
                storage: config::Storage {
                    ab_update: Some(AbUpdate {
                        volume_pairs: Vec::new(),
                    }),
                    ..Default::default()
                },
                ..Default::default()
            },
            servicing_type: None,
            servicing_state: ServicingState::NotProvisioned,
            ..Default::default()
        };

        // 1. If host is in NotProvisioned, update volume is None b/c Trident is not executing any
        // servicing
        assert_eq!(host_status.get_ab_update_volume(), None);

        // 2. If host is in CleanInstallFailed, update volume is None b/c Trident is not executing any
        // servicing
        host_status.servicing_state = ServicingState::CleanInstallFailed;
        assert_eq!(host_status.get_ab_update_volume(), None);

        // 3. If host is in Provisioned, update volume is None b/c Trident is not executing any
        // servicing
        host_status.servicing_state = ServicingState::Provisioned;
        assert_eq!(host_status.get_ab_update_volume(), None);

        // 4. If host is in AbUpdateFailed, update volume is None b/c Trident is not executing any
        // servicing
        host_status.servicing_state = ServicingState::AbUpdateFailed;
        assert_eq!(host_status.get_ab_update_volume(), None);

        // 5. If host is doing CleanInstall, update volume is always A
        host_status.servicing_type = Some(ServicingType::CleanInstall);
        host_status.servicing_state = ServicingState::Staging;
        assert_eq!(
            host_status.get_ab_update_volume(),
            Some(AbVolumeSelection::VolumeA)
        );

        host_status.servicing_state = ServicingState::Staged;
        assert_eq!(
            host_status.get_ab_update_volume(),
            Some(AbVolumeSelection::VolumeA)
        );

        host_status.servicing_state = ServicingState::Finalizing;
        assert_eq!(
            host_status.get_ab_update_volume(),
            Some(AbVolumeSelection::VolumeA)
        );

        host_status.servicing_state = ServicingState::Finalized;
        assert_eq!(
            host_status.get_ab_update_volume(),
            Some(AbVolumeSelection::VolumeA)
        );

        // 6. If host is doing HotPatch, NormalUpdate, or UpdateAndReboot, update volume is always
        // the currently active volume
        host_status.storage.ab_active_volume = Some(AbVolumeSelection::VolumeA);
        host_status.servicing_state = ServicingState::Staging;
        host_status.servicing_type = Some(ServicingType::HotPatch);
        assert_eq!(
            host_status.get_ab_update_volume(),
            host_status.storage.ab_active_volume
        );

        host_status.servicing_type = Some(ServicingType::NormalUpdate);
        assert_eq!(
            host_status.get_ab_update_volume(),
            host_status.storage.ab_active_volume
        );

        host_status.servicing_type = Some(ServicingType::UpdateAndReboot);
        assert_eq!(
            host_status.get_ab_update_volume(),
            host_status.storage.ab_active_volume
        );

        host_status.storage.ab_active_volume = Some(AbVolumeSelection::VolumeB);
        host_status.servicing_type = Some(ServicingType::HotPatch);
        assert_eq!(
            host_status.get_ab_update_volume(),
            host_status.storage.ab_active_volume
        );

        host_status.servicing_type = Some(ServicingType::NormalUpdate);
        assert_eq!(
            host_status.get_ab_update_volume(),
            host_status.storage.ab_active_volume
        );

        host_status.servicing_type = Some(ServicingType::UpdateAndReboot);
        assert_eq!(
            host_status.get_ab_update_volume(),
            host_status.storage.ab_active_volume
        );

        // 7. If host is doing A/B update, update volume is the opposite of the active volume
        host_status.servicing_type = Some(ServicingType::AbUpdate);
        host_status.storage.ab_active_volume = Some(AbVolumeSelection::VolumeA);
        assert_eq!(
            host_status.get_ab_update_volume(),
            Some(AbVolumeSelection::VolumeB)
        );

        // If servicing state changes, the update volume should not change
        host_status.servicing_state = ServicingState::Staged;
        assert_eq!(
            host_status.get_ab_update_volume(),
            Some(AbVolumeSelection::VolumeB)
        );

        host_status.storage.ab_active_volume = Some(AbVolumeSelection::VolumeB);
        assert_eq!(
            host_status.get_ab_update_volume(),
            Some(AbVolumeSelection::VolumeA)
        );

        // If servicing state changes, the update volume should not change
        host_status.servicing_state = ServicingState::Finalizing;
        assert_eq!(
            host_status.get_ab_update_volume(),
            Some(AbVolumeSelection::VolumeA)
        );
    }

    /// Validates logic in get_ab_active_volume() function
    #[test]
    fn test_get_ab_active_volume() {
        let mut host_status = HostStatus {
            spec: HostConfiguration {
                storage: config::Storage {
                    ab_update: Some(AbUpdate {
                        volume_pairs: Vec::new(),
                    }),
                    ..Default::default()
                },
                ..Default::default()
            },
            servicing_type: None,
            servicing_state: ServicingState::NotProvisioned,
            ..Default::default()
        };

        // 1. If host is in NotProvisioned, there is no active volume, as we're still booted from
        // the provisioning OS
        assert_eq!(host_status.get_ab_active_volume(), None);

        // 2. If host is in CleanInstallFailed, there is no active volume either
        host_status.servicing_state = ServicingState::CleanInstallFailed;
        assert_eq!(host_status.get_ab_active_volume(), None);

        // 3. If host is in Provisioned, active volume is the current one
        host_status.servicing_state = ServicingState::Provisioned;
        host_status.storage.ab_active_volume = Some(AbVolumeSelection::VolumeA);
        assert_eq!(
            host_status.get_ab_active_volume(),
            host_status.storage.ab_active_volume
        );

        // 4. If host is in AbUpdateFailed, active volume is the current one
        host_status.servicing_state = ServicingState::AbUpdateFailed;
        host_status.storage.ab_active_volume = Some(AbVolumeSelection::VolumeB);
        assert_eq!(
            host_status.get_ab_active_volume(),
            host_status.storage.ab_active_volume
        );

        // 5. If host is doing CleanInstall, active volume is always None
        host_status.servicing_type = Some(ServicingType::CleanInstall);
        host_status.servicing_state = ServicingState::Staging;
        assert_eq!(host_status.get_ab_active_volume(), None);

        host_status.servicing_state = ServicingState::Staged;
        assert_eq!(host_status.get_ab_active_volume(), None);

        // 6. If host is doing HotPatch, NormalUpdate, UpdateAndReboot, or AbUpdate, the active
        // volume is in host status
        host_status.storage.ab_active_volume = Some(AbVolumeSelection::VolumeA);
        host_status.servicing_type = Some(ServicingType::HotPatch);
        assert_eq!(
            host_status.get_ab_active_volume(),
            host_status.storage.ab_active_volume
        );

        host_status.servicing_type = Some(ServicingType::NormalUpdate);
        assert_eq!(
            host_status.get_ab_active_volume(),
            host_status.storage.ab_active_volume
        );

        host_status.servicing_type = Some(ServicingType::UpdateAndReboot);
        assert_eq!(
            host_status.get_ab_active_volume(),
            host_status.storage.ab_active_volume
        );

        host_status.servicing_type = Some(ServicingType::AbUpdate);
        assert_eq!(
            host_status.get_ab_active_volume(),
            host_status.storage.ab_active_volume
        );

        host_status.storage.ab_active_volume = Some(AbVolumeSelection::VolumeB);
        host_status.servicing_state = ServicingState::Finalizing;
        host_status.servicing_type = Some(ServicingType::HotPatch);
        assert_eq!(
            host_status.get_ab_active_volume(),
            host_status.storage.ab_active_volume
        );

        host_status.servicing_type = Some(ServicingType::NormalUpdate);
        assert_eq!(
            host_status.get_ab_active_volume(),
            host_status.storage.ab_active_volume
        );

        host_status.servicing_type = Some(ServicingType::UpdateAndReboot);
        assert_eq!(
            host_status.get_ab_active_volume(),
            host_status.storage.ab_active_volume
        );

        host_status.servicing_type = Some(ServicingType::AbUpdate);
        assert_eq!(
            host_status.get_ab_active_volume(),
            host_status.storage.ab_active_volume
        );
    }
}
