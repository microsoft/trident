use std::path::Path;

use log::warn;

use trident_api::{
    constants::EFI_DEFAULT_BIN_DIRECTORY, error::TridentError, status::AbVolumeSelection,
};

use super::boot;

const MSHV_DEVICE_PATH: &str = "/dev/mshv";

/// Calls the identity hook only when the MSHV device is present.
pub(crate) fn validate_active_hypervisor(
    install_index: usize,
    expected_volume: Option<AbVolumeSelection>,
) -> Result<(), TridentError> {
    validate_if_mshv_present(Path::new(MSHV_DEVICE_PATH), || {
        validate_hypervisor_identity(install_index, expected_volume)
    })
}

fn validate_if_mshv_present(
    device: &Path,
    check: impl FnOnce() -> Result<(), TridentError>,
) -> Result<(), TridentError> {
    // Presence is a best-effort gate, not evidence of the expected hypervisor identity.
    if device.exists() {
        check()?;
    }
    Ok(())
}

fn validate_hypervisor_identity(
    install_index: usize,
    expected_volume: Option<AbVolumeSelection>,
) -> Result<(), TridentError> {
    let expected_directory = expected_volume
        .map(|volume| boot::make_esp_dir_name(install_index, volume))
        .unwrap_or_else(|| EFI_DEFAULT_BIN_DIRECTORY.into());
    // TODO: Identify the running hypervisor and compare it with the expected OS's MSHV boot components.
    warn!(
        "MSHV hypervisor identity validation is not implemented; assuming success for {:?}",
        expected_directory
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;

    use tempfile::TempDir;

    use trident_api::error::{ErrorKind, ServicingError};

    #[test]
    fn missing_mshv_device_skips_identity_validation() {
        let temp = TempDir::new().unwrap();
        validate_if_mshv_present(&temp.path().join("mshv"), || {
            panic!("Identity validation must not run without the MSHV device")
        })
        .unwrap();
    }

    #[test]
    fn present_mshv_device_runs_identity_validation_and_propagates_failure() {
        let temp = TempDir::new().unwrap();
        let device = temp.path().join("mshv");
        fs::write(&device, []).unwrap();
        let mut checked = false;
        validate_if_mshv_present(&device, || {
            checked = true;
            Ok(())
        })
        .unwrap();
        assert!(checked);

        let error = validate_if_mshv_present(&device, || {
            Err(TridentError::new(
                ServicingError::ValidateAbActiveHypervisor {
                    expected_slot: "AZLB".into(),
                    observed_slot: Some("AZLA".into()),
                },
            ))
        })
        .unwrap_err();
        assert!(matches!(
            error.kind(),
            ErrorKind::Servicing(ServicingError::ValidateAbActiveHypervisor { .. })
        ));
    }
}
