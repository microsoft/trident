use log::warn;

use trident_api::{
    config::HostConfiguration,
    constants::internal_params::{MSHV_SLOT_ESP_PAYLOAD, PRESERVE_MSHV_ESP_PAYLOAD},
    error::{InvalidInputError, TridentError},
    status::{AbVolumeSelection, ServicingType},
};

use super::{boot, EngineContext};

fn unsupported(reason: &str) -> TridentError {
    TridentError::new(InvalidInputError::InvalidInternalParameter {
        name: MSHV_SLOT_ESP_PAYLOAD.into(),
        explanation: reason.into(),
    })
}

pub(crate) fn enabled(spec: &HostConfiguration) -> bool {
    spec.internal_params.get_flag(MSHV_SLOT_ESP_PAYLOAD)
}

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

pub(crate) fn validate_configuration(ctx: &EngineContext) -> Result<(), TridentError> {
    if !enabled(&ctx.spec) {
        return Ok(());
    }
    // x86-64 is an initial support choice while the MSHV payload definition is x64-only.
    // Other architectures should reuse the existing architecture-aware EFI filename helpers.
    if !cfg!(target_arch = "x86_64") || !ctx.image_distro().is_acl() || !ctx.is_uki()? {
        return Err(unsupported(
            "Slot-owned MSHV requires an x86-64 ACL UKI image",
        ));
    }
    if ctx.is_stream_image {
        return Err(unsupported(
            "Slot-owned MSHV requires file-based ESP deployment",
        ));
    }
    if ctx.spec.internal_params.get_flag(PRESERVE_MSHV_ESP_PAYLOAD) {
        return Err(unsupported(
            "Shared-path and slot-owned MSHV modes cannot be combined",
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

/// Hypervisor identity-validation hook for the expected installation and slot.
/// Currently assumes success until runtime identity detection is implemented.
pub(crate) fn validate_active_hypervisor(
    spec: &HostConfiguration,
    install_index: usize,
    expected_volume: Option<AbVolumeSelection>,
) -> Result<(), TridentError> {
    if enabled(spec) {
        let expected_directory =
            expected_volume.map(|volume| boot::make_esp_dir_name(install_index, volume));
        // TODO: Identify the running hypervisor and compare it with the expected OS payload.
        // Deliberately assume success until the identity mechanism is implemented.
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
        assert!(validate_configuration(&ctx).is_ok());
        ctx.spec.storage.encryption = Some(trident_api::config::Encryption {
            pcrs: vec![sysdefs::tpm2::Pcr::Pcr4],
            ..Default::default()
        });
        assert!(validate_configuration(&ctx).is_err());
        ctx.spec.storage.encryption = None;
        ctx.spec.internal_params.set_flag(PRESERVE_MSHV_ESP_PAYLOAD);
        assert!(validate_configuration(&ctx).is_err());
    }
}
