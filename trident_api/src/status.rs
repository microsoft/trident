use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
};

use serde::{Deserialize, Serialize};
use strum::IntoEnumIterator;
use strum_macros::EnumIter;
use uuid::Uuid;

use crate::{
    config::{HostConfiguration, Partition},
    constants::{AB_VOLUME_A_NAME, AB_VOLUME_B_NAME, AZURE_LINUX_INSTALL_ID_PREFIX},
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

    /// Stores the Disks UUID to ID mapping of the host.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub disk_uuid_id_map: HashMap<Uuid, BlockDeviceId>,
}

/// A/B volume selection. Determines which set of volumes are currently
/// active/used by the OS.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, EnumIter)]
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
                    // In host status, servicing type will never be None if servicing state is one
                    // of the above.
                    None => None,
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
                    // In host status, servicing type will never be None if servicing state is one
                    // of the above.
                    None => None,
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

    /// Returns the ESP directory name of the current install's update volume.
    ///
    /// Internally, calls `HostStatus::make_install_id` with the update volume
    /// returned by `HostStatus::get_ab_update_volume` and the current install
    /// index.
    pub fn get_update_esp_dir_name(&self) -> Option<String> {
        Some(Self::make_esp_dir_name(
            self.install_index,
            self.get_ab_update_volume()?,
        ))
    }

    /// Returns an iterator over all possible ESP directory names in ascending
    /// index order. It is used to find the first available install index by
    /// checking for the existence of previous ESP directory names in the ESP
    /// partition.
    ///
    /// **This function should only be used in clean install scenarios, where we
    /// are trying to assess whether there are pre-existing Azure Linux installs
    /// on the host.**
    ///
    /// The iterator will yield tuples of the form `(index, [names...])`, where
    /// `index` is the index of the install, and `names` is an iterator of all the
    /// ESP directory names possible for this index as strings.
    ///
    /// For example, the iterator will yield:
    ///
    /// - (0, ["AZLA", "AZLB"])
    /// - (1, ["AZL2A", "AZL2B"])
    /// - (2, ["AZL3A", "AZL3B"])
    /// - ...
    pub fn make_esp_dir_name_candidates() -> impl Iterator<Item = (usize, Vec<String>)> {
        (0..).map(|idx| {
            (
                idx,
                AbVolumeSelection::iter()
                    .map(move |v| Self::make_esp_dir_name(idx, v))
                    .collect(),
            )
        })
    }

    /// Generate the ESP directory name for a given index and volume selection.
    ///
    /// The ESP directory name ID is a string that is used to uniquely identify
    /// a specific A/B volume on a specific Azure Linux install on a host. As
    /// such, each install may have up to two ESP directory names, one for each
    /// volume.
    ///
    /// The ESP directory name ID is generated as follows:
    ///
    /// - The string starts with the value of `AZURE_LINUX_INSTALL_ID_PREFIX`.
    /// - If this is the first index (0), no number is appended. Otherwise, the
    ///   index is **incremented by 1 to make it 1-indexed** and appended to the
    ///   string.
    /// - Depending on the volume selection, the string is appended with the
    ///   value of either `AB_VOLUME_A_NAME` or `AB_VOLUME_B_NAME`.
    ///
    /// # Arguments
    ///
    /// * `index` - The install index.
    /// * `volume` - The volume selection.
    ///
    /// # Returns
    ///
    /// The ESP directory name ID as a string.
    ///
    /// # Example
    ///
    /// ```
    /// use trident_api::status::{AbVolumeSelection, HostStatus};
    ///
    /// let volume = AbVolumeSelection::VolumeA;
    /// let index = 0;
    /// let install_vol_id = HostStatus::make_esp_dir_name(index, volume);
    /// assert_eq!(install_vol_id, "AZLA".to_owned());
    ///
    /// let volume = AbVolumeSelection::VolumeB;
    /// let index = 1;
    /// let install_vol_id = HostStatus::make_esp_dir_name(index, volume);
    /// assert_eq!(install_vol_id, "AZL2B".to_owned());
    /// ```
    pub fn make_esp_dir_name(index: usize, volume: AbVolumeSelection) -> String {
        format!(
            "{}{}{}",
            AZURE_LINUX_INSTALL_ID_PREFIX,
            match index {
                0 => "".to_owned(),
                _ => (index + 1).to_string(),
            },
            match volume {
                AbVolumeSelection::VolumeA => AB_VOLUME_A_NAME,
                AbVolumeSelection::VolumeB => AB_VOLUME_B_NAME,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use const_format::formatcp;

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

    #[test]
    fn test_make_install_id() {
        // Test for index 0
        assert_eq!(
            HostStatus::make_esp_dir_name(0, AbVolumeSelection::VolumeA),
            formatcp!("{AZURE_LINUX_INSTALL_ID_PREFIX}{AB_VOLUME_A_NAME}")
        );
        assert_eq!(
            HostStatus::make_esp_dir_name(0, AbVolumeSelection::VolumeB),
            formatcp!("{AZURE_LINUX_INSTALL_ID_PREFIX}{AB_VOLUME_B_NAME}")
        );

        // Test for index >0
        for i in 1..2000 {
            for vol in AbVolumeSelection::iter() {
                assert_eq!(
                    HostStatus::make_esp_dir_name(i, vol),
                    format!(
                        "{AZURE_LINUX_INSTALL_ID_PREFIX}{}{}",
                        i + 1,
                        match vol {
                            AbVolumeSelection::VolumeA => AB_VOLUME_A_NAME,
                            AbVolumeSelection::VolumeB => AB_VOLUME_B_NAME,
                        }
                    )
                );
            }
        }
    }

    #[test]
    fn test_make_install_volume_id_candidates() {
        let mut candidates = HostStatus::make_esp_dir_name_candidates();

        // Test for index 0
        let first = candidates.next().unwrap();
        assert_eq!(
            first,
            (
                0,
                vec![
                    format!("{AZURE_LINUX_INSTALL_ID_PREFIX}{AB_VOLUME_A_NAME}"),
                    format!("{AZURE_LINUX_INSTALL_ID_PREFIX}{AB_VOLUME_B_NAME}"),
                ]
            )
        );

        // Test for index >0
        for i in 1..100 {
            let candidate = candidates.next().unwrap();
            assert_eq!(
                candidate,
                (
                    i,
                    vec![
                        format!("{AZURE_LINUX_INSTALL_ID_PREFIX}{}{AB_VOLUME_A_NAME}", i + 1),
                        format!("{AZURE_LINUX_INSTALL_ID_PREFIX}{}{AB_VOLUME_B_NAME}", i + 1),
                    ]
                )
            );
        }
    }

    /// Tests setting the index and getting the corresponding install ID
    #[test]
    fn test_set_get_install() {
        // Test for clean install
        let mut host_status = HostStatus {
            servicing_type: Some(ServicingType::CleanInstall),
            servicing_state: ServicingState::Staging,
            ..Default::default()
        };

        host_status.install_index = 0;
        assert_eq!(
            host_status.get_update_esp_dir_name(),
            Some(format!("{AZURE_LINUX_INSTALL_ID_PREFIX}{AB_VOLUME_A_NAME}"))
        );
        host_status.install_index = 1;
        assert_eq!(
            host_status.get_update_esp_dir_name(),
            Some(format!(
                "{AZURE_LINUX_INSTALL_ID_PREFIX}2{AB_VOLUME_A_NAME}"
            ))
        );
        host_status.install_index = 200;
        assert_eq!(
            host_status.get_update_esp_dir_name(),
            Some(format!(
                "{AZURE_LINUX_INSTALL_ID_PREFIX}201{AB_VOLUME_A_NAME}"
            ))
        );

        // Test for update to A
        let mut host_status = HostStatus {
            servicing_type: Some(ServicingType::AbUpdate),
            servicing_state: ServicingState::Staging,
            storage: Storage {
                ab_active_volume: Some(AbVolumeSelection::VolumeB),
                ..Default::default()
            },
            ..Default::default()
        };

        host_status.install_index = 0;
        assert_eq!(
            host_status.get_update_esp_dir_name(),
            Some(format!("{AZURE_LINUX_INSTALL_ID_PREFIX}{AB_VOLUME_A_NAME}"))
        );
        host_status.install_index = 1;
        assert_eq!(
            host_status.get_update_esp_dir_name(),
            Some(format!(
                "{AZURE_LINUX_INSTALL_ID_PREFIX}2{AB_VOLUME_A_NAME}"
            ))
        );
        host_status.install_index = 200;
        assert_eq!(
            host_status.get_update_esp_dir_name(),
            Some(format!(
                "{AZURE_LINUX_INSTALL_ID_PREFIX}201{AB_VOLUME_A_NAME}"
            ))
        );

        // Test for update to B
        let mut host_status = HostStatus {
            servicing_type: Some(ServicingType::AbUpdate),
            servicing_state: ServicingState::Staging,
            storage: Storage {
                ab_active_volume: Some(AbVolumeSelection::VolumeA),
                ..Default::default()
            },
            ..Default::default()
        };

        host_status.install_index = 0;
        assert_eq!(
            host_status.get_update_esp_dir_name(),
            Some(format!("{AZURE_LINUX_INSTALL_ID_PREFIX}{AB_VOLUME_B_NAME}"))
        );
        host_status.install_index = 1;
        assert_eq!(
            host_status.get_update_esp_dir_name(),
            Some(format!(
                "{AZURE_LINUX_INSTALL_ID_PREFIX}2{AB_VOLUME_B_NAME}"
            ))
        );
        host_status.install_index = 200;
        assert_eq!(
            host_status.get_update_esp_dir_name(),
            Some(format!(
                "{AZURE_LINUX_INSTALL_ID_PREFIX}201{AB_VOLUME_B_NAME}"
            ))
        );
    }
}
