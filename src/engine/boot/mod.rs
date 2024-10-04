use std::path::Path;

use strum::IntoEnumIterator;

use trident_api::{
    constants::{AB_VOLUME_A_NAME, AB_VOLUME_B_NAME, AZURE_LINUX_INSTALL_ID_PREFIX},
    error::{ReportError, ServicingError, TridentError},
    status::{AbVolumeSelection, HostStatus},
};

use crate::engine::Subsystem;

pub(super) mod esp;
pub(super) mod grub;

#[derive(Default, Debug)]
pub(super) struct BootSubsystem;
impl Subsystem for BootSubsystem {
    fn name(&self) -> &'static str {
        "boot"
    }

    #[tracing::instrument(name = "boot_provision", skip_all)]
    fn provision(
        &mut self,
        host_status: &HostStatus,
        mount_point: &Path,
    ) -> Result<(), TridentError> {
        // Perform file-based deployment of ESP images, if needed, after filesystems have been
        // mounted and initialized
        esp::deploy_esp_images(host_status, mount_point)
            .structured(ServicingError::DeployESPImages)?;

        Ok(())
    }

    #[tracing::instrument(name = "boot_configuration", skip_all)]
    fn configure(
        &mut self,
        host_status: &HostStatus,
        _exec_root: &Path,
    ) -> Result<(), TridentError> {
        grub::update_configs(host_status).structured(ServicingError::UpdateGrubConfigs)?;

        Ok(())
    }
}

/// Returns the ESP directory name of the current install's update volume.
///
/// Internally, calls `HostStatus::make_install_id` with the update volume
/// returned by `HostStatus::get_ab_update_volume` and the current install
/// index.
pub fn get_update_esp_dir_name(host_status: &HostStatus) -> Option<String> {
    Some(make_esp_dir_name(
        host_status.install_index,
        host_status.get_ab_update_volume()?,
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
                .map(move |v| make_esp_dir_name(idx, v))
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
/// ```rust,ignore
/// use trident_api::status::{AbVolumeSelection, HostStatus};
///
/// let volume = AbVolumeSelection::VolumeA;
/// let index = 0;
/// let install_vol_id = make_esp_dir_name(index, volume);
/// assert_eq!(install_vol_id, "AZLA".to_owned());
///
/// let volume = AbVolumeSelection::VolumeB;
/// let index = 1;
/// let install_vol_id = make_esp_dir_name(index, volume);
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

#[cfg(test)]
mod tests {
    use super::*;

    use const_format::formatcp;

    use trident_api::status::{ServicingState, ServicingType};

    #[test]
    fn test_make_install_id() {
        // Test for index 0
        assert_eq!(
            make_esp_dir_name(0, AbVolumeSelection::VolumeA),
            formatcp!("{AZURE_LINUX_INSTALL_ID_PREFIX}{AB_VOLUME_A_NAME}")
        );
        assert_eq!(
            make_esp_dir_name(0, AbVolumeSelection::VolumeB),
            formatcp!("{AZURE_LINUX_INSTALL_ID_PREFIX}{AB_VOLUME_B_NAME}")
        );

        // Test for index >0
        for i in 1..2000 {
            for vol in AbVolumeSelection::iter() {
                assert_eq!(
                    make_esp_dir_name(i, vol),
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
        let mut candidates = make_esp_dir_name_candidates();

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
            servicing_type: ServicingType::CleanInstall,
            servicing_state: ServicingState::Staging,
            ..Default::default()
        };

        host_status.install_index = 0;
        assert_eq!(
            get_update_esp_dir_name(&host_status),
            Some(format!("{AZURE_LINUX_INSTALL_ID_PREFIX}{AB_VOLUME_A_NAME}"))
        );
        host_status.install_index = 1;
        assert_eq!(
            get_update_esp_dir_name(&host_status),
            Some(format!(
                "{AZURE_LINUX_INSTALL_ID_PREFIX}2{AB_VOLUME_A_NAME}"
            ))
        );
        host_status.install_index = 200;
        assert_eq!(
            get_update_esp_dir_name(&host_status),
            Some(format!(
                "{AZURE_LINUX_INSTALL_ID_PREFIX}201{AB_VOLUME_A_NAME}"
            ))
        );

        // Test for update to A
        let mut host_status = HostStatus {
            servicing_type: ServicingType::AbUpdate,
            servicing_state: ServicingState::Staging,
            ab_active_volume: Some(AbVolumeSelection::VolumeB),
            ..Default::default()
        };

        host_status.install_index = 0;
        assert_eq!(
            get_update_esp_dir_name(&host_status),
            Some(format!("{AZURE_LINUX_INSTALL_ID_PREFIX}{AB_VOLUME_A_NAME}"))
        );
        host_status.install_index = 1;
        assert_eq!(
            get_update_esp_dir_name(&host_status),
            Some(format!(
                "{AZURE_LINUX_INSTALL_ID_PREFIX}2{AB_VOLUME_A_NAME}"
            ))
        );
        host_status.install_index = 200;
        assert_eq!(
            get_update_esp_dir_name(&host_status),
            Some(format!(
                "{AZURE_LINUX_INSTALL_ID_PREFIX}201{AB_VOLUME_A_NAME}"
            ))
        );

        // Test for update to B
        let mut host_status = HostStatus {
            servicing_type: ServicingType::AbUpdate,
            servicing_state: ServicingState::Staging,
            ab_active_volume: Some(AbVolumeSelection::VolumeA),
            ..Default::default()
        };

        host_status.install_index = 0;
        assert_eq!(
            get_update_esp_dir_name(&host_status),
            Some(format!("{AZURE_LINUX_INSTALL_ID_PREFIX}{AB_VOLUME_B_NAME}"))
        );
        host_status.install_index = 1;
        assert_eq!(
            get_update_esp_dir_name(&host_status),
            Some(format!(
                "{AZURE_LINUX_INSTALL_ID_PREFIX}2{AB_VOLUME_B_NAME}"
            ))
        );
        host_status.install_index = 200;
        assert_eq!(
            get_update_esp_dir_name(&host_status),
            Some(format!(
                "{AZURE_LINUX_INSTALL_ID_PREFIX}201{AB_VOLUME_B_NAME}"
            ))
        );
    }
}
