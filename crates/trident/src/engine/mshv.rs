use std::path::Path;

use trident_api::{error::TridentError, status::AbVolumeSelection};

use super::boot;

const MSHV_DEVICE_PATH: &str = "/dev/mshv";

/// Applies EFI loader-origin validation when /dev/mshv reports as present.
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
        boot::uki::validate_efi_loader_origin(install_index, expected_volume)?;
    }
    Ok(())
}
