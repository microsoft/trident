use std::path::Path;

use log::warn;

use osutils::efivar;
use trident_api::{
    error::{ServicingError, TridentError},
    status::AbVolumeSelection,
};

use super::boot;

const MSHV_DEVICE_PATH: &str = "/dev/mshv";

/// Validates expected-slot identity when /dev/mshv reports as present.
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
    if Path::new(MSHV_DEVICE_PATH).exists() {
        validate_hypervisor_identity(install_index, expected_volume)?;
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
    let image = match efivar::read_loader_image_identifier() {
        Ok(image) => image,
        Err(error) => {
            // Missing evidence must not be reported as a rejected A/B trial.
            warn!("Cannot determine MSHV boot-chain origin; skipping validation: {error:?}");
            return Ok(());
        }
    };
    validate_loader_origin(install_index, expected_volume, &image)
}

/// Checks a loader-image identifier against the expected slot for `install_index`.
///
/// Normalizes EFI separators and ASCII case, recognizing only this installation's
/// complete slot-local `second.efi` paths. Unsupported origins, including fallback
/// paths, warn and succeed. Only a recognized opposite A/B slot returns a mismatch
/// error; success does not attest the running payload.
fn validate_loader_origin(
    install_index: usize,
    expected_volume: AbVolumeSelection,
    image: &str,
) -> Result<(), TridentError> {
    let image = image.replace('/', "\\");

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

    use trident_api::error::ErrorKind;

    #[test]
    fn matching_loader_origins_accept_efi_case_and_separators() {
        for (index, volume, image) in [
            (0, AbVolumeSelection::VolumeA, r"\EFI\AZLA\second.efi"),
            (0, AbVolumeSelection::VolumeB, "/efi/azlb/SECOND.EFI"),
            (1, AbVolumeSelection::VolumeA, r"\EFI\AZL2A\second.efi"),
            (2, AbVolumeSelection::VolumeB, r"\EFI\AZL3B\second.efi"),
        ] {
            validate_loader_origin(index, volume, image).unwrap();
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
            let error = validate_loader_origin(index, volume, image).unwrap_err();
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
            validate_loader_origin(0, AbVolumeSelection::VolumeA, image).unwrap();
        }
        validate_hypervisor_identity(0, None).unwrap();
    }
}
