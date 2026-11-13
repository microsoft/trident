//! Slot-owned MSHV validation and efitreeloader (ETL) ESP definitions.

use sysdefs::arch::SystemArchitecture;
use trident_api::{error::TridentError, status::ServicingType};

use crate::engine::{
    mshv::{enabled, unsupported, validate_transition},
    EngineContext,
};

/// Filename of the second-stage EFI executable for efitreeloader (ETL).
pub(super) const ETL_SECOND_STAGE_EFI_FILENAME: &str = "second.efi";

/// Enforces support and mode-transition restrictions when slot-owned MSHV is enabled.
pub(super) fn validate_configuration(ctx: &EngineContext) -> Result<(), TridentError> {
    if !enabled(&ctx.spec) {
        return Ok(());
    }
    // The current payload allowlist supports only x86-64.
    if !SystemArchitecture::current().is_x64() || !ctx.image_distro().is_acl() || !ctx.is_uki()? {
        return Err(unsupported(
            "Slot-owned MSHV requires an x86-64 ACL UKI image",
        ));
    }
    if ctx.is_stream_image {
        return Err(unsupported(
            "Slot-owned MSHV requires file-based ESP deployment",
        ));
    }
    if ctx
        .spec
        .storage
        .encryption
        .as_ref()
        .is_some_and(|encryption| !encryption.pcrs.is_empty())
    {
        return Err(unsupported(
            "Slot-owned MSHV TPM/PCR-bound encryption is not validated",
        ));
    }
    if ctx.servicing_type != ServicingType::CleanInstall {
        validate_transition(&ctx.spec_old, &ctx.spec)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use trident_api::constants::internal_params::MSHV_SLOT_ESP_PAYLOAD;

    /// Unsupported MSHV combinations fail only after opt-in.
    #[test]
    fn rejects_unsupported_mshv_images_without_affecting_other_images() {
        let mut ctx = EngineContext {
            is_stream_image: true,
            is_uki: Some(true),
            servicing_type: ServicingType::CleanInstall,
            ..Default::default()
        }
        .with_image(crate::osimage::mock::MockOsImage {
            os_release: osutils::osrelease::OsRelease {
                id: Some(osutils::osrelease::AZURE_LINUX_DISTRO_ID.into()),
                variant_id: Some(osutils::osrelease::AZURE_CONTAINER_LINUX_VARIANT_ID.into()),
                ..Default::default()
            },
            is_uki: true,
            ..crate::osimage::mock::MockOsImage::new()
        });
        assert!(validate_configuration(&ctx).is_ok());
        ctx.spec.internal_params.set_flag(MSHV_SLOT_ESP_PAYLOAD);
        assert!(validate_configuration(&ctx).is_err());
        ctx.is_stream_image = false;
        assert_eq!(
            validate_configuration(&ctx).is_ok(),
            SystemArchitecture::current().is_x64(),
        );
        ctx.spec.storage.encryption = Some(trident_api::config::Encryption {
            pcrs: vec![sysdefs::tpm2::Pcr::Pcr4],
            ..Default::default()
        });
        assert!(validate_configuration(&ctx).is_err());
    }
}
