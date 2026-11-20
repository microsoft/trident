use std::path::Path;

use log::warn;

use osutils::efivar;
use trident_api::{
    error::{ServicingError, TridentError},
    status::AbVolumeSelection,
};

use super::boot;

const MSHV_DEVICE_PATH: &str = "/dev/mshv";

/// Runs the expected-slot identity hook when /dev/mshv reports as present.
///
/// `expected_volume` selects the A/B slot for `install_index`; None skips
/// fallback identity validation. Missing/uninspectable devices and unavailable
/// or unsupported loader metadata also skip the check successfully.
///
/// A managed loader-directory mismatch propagates to the caller's recovery flow.
/// Success is not payload attestation: this infers slot consistency from the
/// slot-owned boot chain, without inspecting ESP files or the running binaries.
pub(super) fn validate_active_hypervisor(
    install_index: usize,
    expected_volume: Option<AbVolumeSelection>,
) -> Result<(), TridentError> {
    validate_if_mshv_present(Path::new(MSHV_DEVICE_PATH), || {
        validate_hypervisor_identity(install_index, expected_volume)
    })
}

/// Applies a best-effort path-presence gate to an identity check.
///
/// Path existence does not establish device type or hypervisor identity.
/// Missing/uninspectable paths return success without invoking `check`;
/// otherwise the callback runs once and its failure is propagated.
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

/// Compares the loader's managed-slot origin with the validated OS expectation.
fn validate_hypervisor_identity(
    install_index: usize,
    expected_volume: Option<AbVolumeSelection>,
) -> Result<(), TridentError> {
    let Some(expected_volume) = expected_volume else {
        return Ok(());
    };
    validate_loader_origin(
        install_index,
        expected_volume,
        efivar::read_loader_image_identifier(),
    )
}

/// Checks a loader-image read result against the expected slot for `install_index`.
///
/// Normalizes EFI separators and ASCII case, recognizing only this installation's
/// complete slot-local `second.efi` paths. Unavailable or unsupported evidence,
/// including fallback paths, warns and succeeds. Only a recognized opposite A/B
/// slot returns a mismatch error; success does not attest the running payload.
fn validate_loader_origin(
    install_index: usize,
    expected_volume: AbVolumeSelection,
    image: Result<String, TridentError>,
) -> Result<(), TridentError> {
    let image = match image {
        Ok(image) => image.replace('/', "\\"),
        Err(error) => {
            // Missing evidence must not be reported as a rejected A/B trial.
            warn!("Cannot determine MSHV boot-chain origin; skipping validation: {error:?}");
            return Ok(());
        }
    };

    let observed_directory = [AbVolumeSelection::VolumeA, AbVolumeSelection::VolumeB]
        .into_iter()
        .map(|volume| boot::make_esp_dir_name(install_index, volume))
        .find(|directory| image.eq_ignore_ascii_case(&format!(r"\EFI\{directory}\second.efi")));
    let Some(observed_directory) = observed_directory else {
        warn!("Unsupported MSHV boot-chain origin {image:?}; skipping validation");
        return Ok(());
    };

    let expected_directory = boot::make_esp_dir_name(install_index, expected_volume);
    if observed_directory != expected_directory {
        return Err(TridentError::new(
            ServicingError::ValidateAbActiveHypervisor {
                expected_slot: expected_directory,
                observed_slot: Some(observed_directory),
            },
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;

    use tempfile::TempDir;

    use trident_api::error::ErrorKind;

    #[test]
    fn matching_loader_origins_accept_efi_case_and_separators() {
        for (index, volume, image) in [
            (0, AbVolumeSelection::VolumeA, r"\EFI\AZLA\second.efi"),
            (0, AbVolumeSelection::VolumeB, "/efi/azlb/SECOND.EFI"),
            (1, AbVolumeSelection::VolumeA, r"\EFI\AZL2A\second.efi"),
            (2, AbVolumeSelection::VolumeB, r"\EFI\AZL3B\second.efi"),
        ] {
            validate_loader_origin(index, volume, Ok(image.into())).unwrap();
        }
    }

    #[test]
    fn opposite_loader_slot_returns_structured_mismatch() {
        for (index, volume, image, expected, observed) in [
            (
                0,
                AbVolumeSelection::VolumeA,
                r"\EFI\AZLB\second.efi",
                "AZLA",
                "AZLB",
            ),
            (
                0,
                AbVolumeSelection::VolumeB,
                "/efi/azla/SECOND.EFI",
                "AZLB",
                "AZLA",
            ),
            (
                1,
                AbVolumeSelection::VolumeB,
                r"\EFI\AZL2A\second.efi",
                "AZL2B",
                "AZL2A",
            ),
        ] {
            let error = validate_loader_origin(index, volume, Ok(image.into())).unwrap_err();
            assert!(
                matches!(
                    error.kind(),
                    ErrorKind::Servicing(ServicingError::ValidateAbActiveHypervisor {
                        expected_slot,
                        observed_slot,
                    }) if expected_slot == expected && observed_slot.as_deref() == Some(observed)
                ),
                "got {error:?}"
            );
        }
    }

    #[test]
    fn unsupported_loader_origins_do_not_reject_trial() {
        for image in [
            r"\EFI\BOOT\second.efi",
            r"\EFI\AZL2B\second.efi",
            r"\EFI\AZLB\bootx64.efi",
            r"\EFI\AZLB\second.efi.extra",
            r"\EFI\AZLB\second.efi\extra",
            r"\EFI\other\second.efi",
            r"EFI\AZLB\second.efi",
            "",
        ] {
            validate_loader_origin(0, AbVolumeSelection::VolumeA, Ok(image.into())).unwrap();
        }
        validate_hypervisor_identity(0, None).unwrap();
    }

    #[test]
    fn unavailable_loader_metadata_does_not_reject_trial() {
        validate_loader_origin(
            0,
            AbVolumeSelection::VolumeB,
            Err(TridentError::new(ServicingError::ReadEfiVariable {
                name: "LoaderImageIdentifier".into(),
            })),
        )
        .unwrap();
    }

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
