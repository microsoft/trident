use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
};

use serde::{Deserialize, Serialize};
use strum_macros::EnumIter;
use uuid::Uuid;

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
    pub servicing_type: ServicingType,

    /// Current state of the servicing that Trident is executing on the host.
    pub servicing_state: ServicingState,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<serde_yaml::Value>,

    /// The path associated with each block device in the Host Configuration.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub block_device_paths: BTreeMap<BlockDeviceId, PathBuf>,

    /// A/B update status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ab_active_volume: Option<AbVolumeSelection>,

    /// Stores the Disks UUID to ID mapping of the host.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub disks_by_uuid: HashMap<Uuid, BlockDeviceId>,

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
}

/// ServicingType is the type of servicing that the Trident agent is executing on the host. Through
/// ServicingType, Trident communicates what servicing operations are in progress.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
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
    /// No servicing is currently in progress.
    #[default]
    NoActiveServicing = 5,
}

/// ServicingState describes the progress of the servicing that the Trident agent is executing on
/// the host. The host will transition through a different sequence of servicing states, depending
/// on the servicing type.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
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

/// A/B volume selection. Determines which set of volumes are currently
/// active/used by the OS.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, EnumIter)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum AbVolumeSelection {
    VolumeA,
    VolumeB,
}

impl HostStatus {
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
            servicing_type: ServicingType::NoActiveServicing,
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
        host_status.servicing_type = ServicingType::CleanInstall;
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

        host_status.servicing_state = ServicingState::Finalized;
        assert_eq!(
            host_status.get_ab_update_volume(),
            Some(AbVolumeSelection::VolumeA)
        );

        // 6. If host is doing HotPatch, NormalUpdate, or UpdateAndReboot, update volume is always
        // the currently active volume
        host_status.ab_active_volume = Some(AbVolumeSelection::VolumeA);
        host_status.servicing_state = ServicingState::Staging;
        host_status.servicing_type = ServicingType::HotPatch;
        assert_eq!(
            host_status.get_ab_update_volume(),
            host_status.ab_active_volume
        );

        host_status.servicing_type = ServicingType::NormalUpdate;
        assert_eq!(
            host_status.get_ab_update_volume(),
            host_status.ab_active_volume
        );

        host_status.servicing_type = ServicingType::UpdateAndReboot;
        assert_eq!(
            host_status.get_ab_update_volume(),
            host_status.ab_active_volume
        );

        host_status.ab_active_volume = Some(AbVolumeSelection::VolumeB);
        host_status.servicing_type = ServicingType::HotPatch;
        assert_eq!(
            host_status.get_ab_update_volume(),
            host_status.ab_active_volume
        );

        host_status.servicing_type = ServicingType::NormalUpdate;
        assert_eq!(
            host_status.get_ab_update_volume(),
            host_status.ab_active_volume
        );

        host_status.servicing_type = ServicingType::UpdateAndReboot;
        assert_eq!(
            host_status.get_ab_update_volume(),
            host_status.ab_active_volume
        );

        // 7. If host is doing A/B update, update volume is the opposite of the active volume
        host_status.servicing_type = ServicingType::AbUpdate;
        host_status.ab_active_volume = Some(AbVolumeSelection::VolumeA);
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

        host_status.ab_active_volume = Some(AbVolumeSelection::VolumeB);
        assert_eq!(
            host_status.get_ab_update_volume(),
            Some(AbVolumeSelection::VolumeA)
        );
    }
}
