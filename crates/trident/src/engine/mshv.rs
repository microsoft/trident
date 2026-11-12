use log::warn;

use trident_api::{
    config::HostConfiguration,
    constants::internal_params::MSHV_SLOT_ESP_PAYLOAD,
    error::{InvalidInputError, TridentError},
    status::AbVolumeSelection,
};

use super::boot;

/// Attributes unsupported configurations to the MSHV mode parameter.
pub(crate) fn unsupported(reason: &str) -> TridentError {
    TridentError::new(InvalidInputError::InvalidInternalParameter {
        name: MSHV_SLOT_ESP_PAYLOAD.into(),
        explanation: reason.into(),
    })
}

/// Checks the slot-owned MSHV opt-in.
pub(crate) fn enabled(spec: &HostConfiguration) -> bool {
    spec.internal_params.get_flag(MSHV_SLOT_ESP_PAYLOAD)
}

/// Rejects MSHV mode changes on an existing installation.
pub(crate) fn validate_transition(
    previous: &HostConfiguration,
    requested: &HostConfiguration,
) -> Result<(), TridentError> {
    if enabled(previous) != enabled(requested) {
        return Err(unsupported(
            "Changing slot-owned MSHV mode on an existing installation is not supported",
        ));
    }
    Ok(())
}

/// Success stub: logs the expected slot only when MSHV is enabled.
/// Does not inspect hypervisor identity.
pub(crate) fn validate_active_hypervisor(
    spec: &HostConfiguration,
    install_index: usize,
    expected_volume: Option<AbVolumeSelection>,
) -> Result<(), TridentError> {
    if enabled(spec) {
        let expected_directory =
            expected_volume.map(|volume| boot::make_esp_dir_name(install_index, volume));
        // TODO: Identify the running hypervisor and compare it with the expected OS payload.
        warn!(
            "MSHV hypervisor identity validation is not implemented; assuming success for {:?}",
            expected_directory
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Matching modes pass; transitions fail in either direction.
    #[test]
    fn rejects_mode_changes_but_allows_matching_installation_modes() {
        let ordinary = HostConfiguration::default();
        let mut mshv = ordinary.clone();
        mshv.internal_params.set_flag(MSHV_SLOT_ESP_PAYLOAD);
        assert!(validate_transition(&ordinary, &mshv).is_err());
        assert!(validate_transition(&mshv, &ordinary).is_err());
        assert!(validate_transition(&ordinary, &ordinary).is_ok());
        assert!(validate_transition(&mshv, &mshv).is_ok());
    }
}
