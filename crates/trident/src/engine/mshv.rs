use log::warn;

use osutils::container;
use trident_api::{
    config::HostConfiguration,
    constants::{EFI_DEFAULT_BIN_DIRECTORY, ESP_EFI_DIRECTORY},
    error::{InvalidInputError, ReportError, TridentError, TridentResultExt},
    status::AbVolumeSelection,
};

use super::boot::{self, mshv as boot_mshv};

/// Checks the expected boot bundle, but assumes its hypervisor identity matches.
pub(crate) fn validate_active_hypervisor(
    spec: &HostConfiguration,
    install_index: usize,
    expected_volume: Option<AbVolumeSelection>,
) -> Result<(), TridentError> {
    let Some(esp_path) = spec
        .storage
        .filesystems
        .iter()
        .find(|filesystem| filesystem.is_esp)
        .and_then(|filesystem| filesystem.mount_point_path())
    else {
        return Ok(());
    };
    let esp_path = container::get_host_relative_path(esp_path.to_path_buf())
        .message("Failed to resolve ESP path for MSHV validation")?;
    let expected_directory = expected_volume
        .map(|volume| boot::make_esp_dir_name(install_index, volume))
        .unwrap_or_else(|| EFI_DEFAULT_BIN_DIRECTORY.into());
    // No COSI is loaded; inspect the expected slot, or BOOT if no slot is selected.
    if boot_mshv::has_slot_mshv_bundle(&esp_path.join(ESP_EFI_DIRECTORY).join(&expected_directory))
        .structured(InvalidInputError::InvalidBootConfiguration)?
    {
        // TODO: Identify the running hypervisor and compare it with the expected OS payload.
        warn!(
            "MSHV hypervisor identity validation is not implemented; assuming success for {:?}",
            expected_directory
        );
    }
    Ok(())
}
