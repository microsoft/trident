//! Validates file-based MSHV support and retained-slot compatibility.

use std::path::Path;

use anyhow::{anyhow, ensure, Context, Error};

use osutils::container;
use sysdefs::arch::SystemArchitecture;
use trident_api::{
    constants::ESP_EFI_DIRECTORY,
    error::{InvalidInputError, ReportError, TridentError},
    status::ServicingType,
};

use crate::{
    engine::{
        boot::{self, mshv as boot_mshv},
        EngineContext,
    },
    osimage::bootloader::Bootloader,
};

/// Checks file-based MSHV support and A/B layout compatibility before staging.
///
/// Streamed deployments are skipped. Declared MSHV images require an x86-64
/// build, an ACL UKI, and no PCR-bound encryption. All file-based A/B updates
/// must match the retained slot's MSHV mode, using the host ESP when containerized.
/// Does not modify boot files or validate the running hypervisor.
pub(super) fn validate_configuration(ctx: &EngineContext) -> Result<(), TridentError> {
    if ctx.is_stream_image {
        return Ok(());
    }
    if ctx.bootloader().is_some_and(Bootloader::uses_mshv) {
        // The required bootloader files currently support only x86-64.
        if !SystemArchitecture::current().is_x64()
            || !ctx.image_distro().is_acl()
            || !ctx.is_uki()?
        {
            return Err(TridentError::with_source(
                InvalidInputError::InvalidBootConfiguration,
                anyhow!("Slot-owned MSHV requires an x86-64 ACL UKI image"),
            ));
        }
        if ctx
            .spec
            .storage
            .encryption
            .as_ref()
            .is_some_and(|encryption| !encryption.pcrs.is_empty())
        {
            return Err(TridentError::with_source(
                InvalidInputError::InvalidBootConfiguration,
                anyhow!("Slot-owned MSHV TPM/PCR-bound encryption is not validated"),
            ));
        }
    }
    if ctx.servicing_type == ServicingType::AbUpdate {
        let esp_path = container::get_host_relative_path(ctx.esp_mount_path.clone())?;
        validate_transition(ctx, &esp_path)
            .structured(InvalidInputError::InvalidBootConfiguration)?;
    }
    Ok(())
}

/// Requires an A/B candidate's MSHV metadata to match the retained boot tree.
///
/// `esp_path` must identify the host ESP. The retained directory comes from
/// the installation index and active A/B volume, not the fallback BOOT tree.
/// Non-A/B operations are skipped; missing volume selection, malformed MSHV
/// components, inspection errors or a mode change fail validation.
fn validate_transition(ctx: &EngineContext, esp_path: &Path) -> Result<(), Error> {
    if ctx.servicing_type != ServicingType::AbUpdate {
        return Ok(());
    }
    let active_volume = ctx
        .ab_active_volume
        .context("A/B active volume is not set")?;
    let active_boot_dir = esp_path
        .join(ESP_EFI_DIRECTORY)
        .join(boot::make_esp_dir_name(ctx.install_index, active_volume));
    ensure!(
        boot_mshv::has_slot_mshv_boot_components(&active_boot_dir)?
            == ctx.bootloader().is_some_and(Bootloader::uses_mshv),
        "Changing slot-owned MSHV mode on an existing installation is not supported"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;

    use tempfile::TempDir;

    use osutils::osrelease::{OsRelease, AZURE_CONTAINER_LINUX_VARIANT_ID, AZURE_LINUX_DISTRO_ID};
    use sysdefs::tpm2::Pcr;
    use trident_api::{config::Encryption, status::AbVolumeSelection};

    use crate::{
        engine::{boot::mshv::MSHV_SLOT_BOOT_FILES, Subsystem},
        osimage::{bootloader::BootloaderType, mock::MockOsImage},
        subsystems::esp::EspSubsystem,
    };

    /// Only file-based deployment of metadata-declared MSHV images receives these restrictions.
    #[test]
    fn rejects_unsupported_mshv_images_without_affecting_other_images() {
        let image = MockOsImage {
            os_release: OsRelease {
                id: Some(AZURE_LINUX_DISTRO_ID.into()),
                variant_id: Some(AZURE_CONTAINER_LINUX_VARIANT_ID.into()),
                ..Default::default()
            },
            is_uki: true,
            ..MockOsImage::new()
        };
        let mut ctx = EngineContext {
            is_stream_image: true,
            is_uki: Some(true),
            servicing_type: ServicingType::CleanInstall,
            ..Default::default()
        }
        .with_image(image);
        assert!(validate_configuration(&ctx).is_ok());
        ctx.bootloader = Some(Bootloader::new(
            BootloaderType::EfitreeloaderMshvSystemdBoot,
            None,
        ));
        assert!(validate_configuration(&ctx).is_ok());
        ctx.is_stream_image = false;
        assert_eq!(
            validate_configuration(&ctx).is_ok(),
            SystemArchitecture::current().is_x64(),
        );
        ctx.spec.storage.encryption = Some(Encryption {
            pcrs: vec![Pcr::Pcr4],
            ..Default::default()
        });
        assert!(validate_configuration(&ctx).is_err());
        ctx.is_stream_image = true;
        assert!(validate_configuration(&ctx).is_ok());
    }

    /// Source metadata must match the retained slot, including non-default slots.
    #[test]
    fn rejects_mshv_layout_changes() {
        for retained_mshv in [false, true] {
            let esp = TempDir::new().unwrap();
            if retained_mshv {
                for name in MSHV_SLOT_BOOT_FILES {
                    let path = esp.path().join("EFI/AZL2B").join(name);
                    fs::create_dir_all(path.parent().unwrap()).unwrap();
                    fs::write(path, b"payload").unwrap();
                }
            }
            let mut ctx = EngineContext {
                servicing_type: ServicingType::AbUpdate,
                install_index: 1,
                ab_active_volume: Some(AbVolumeSelection::VolumeB),
                esp_mount_path: esp.path().to_path_buf(),
                is_uki: Some(true),
                ..Default::default()
            }
            .with_image(MockOsImage {
                os_release: OsRelease {
                    id: Some(AZURE_LINUX_DISTRO_ID.into()),
                    variant_id: Some(AZURE_CONTAINER_LINUX_VARIANT_ID.into()),
                    ..Default::default()
                },
                is_uki: true,
                ..MockOsImage::new()
            });
            for requested_mshv in [false, true] {
                let bootloader_type = if requested_mshv {
                    BootloaderType::EfitreeloaderMshvSystemdBoot
                } else {
                    BootloaderType::SystemdBoot
                };
                ctx.bootloader = Some(Bootloader::new(bootloader_type, None));
                assert_eq!(
                    EspSubsystem.validate_host_config(&ctx).is_ok(),
                    retained_mshv == requested_mshv
                        && (!requested_mshv || SystemArchitecture::current().is_x64())
                );
            }
            ctx.is_stream_image = true;
            EspSubsystem.validate_host_config(&ctx).unwrap();
            ctx.is_stream_image = false;
            ctx.bootloader = Some(Bootloader::new(BootloaderType::SystemdBoot, None));
            ctx.servicing_type = ServicingType::RuntimeUpdate;
            EspSubsystem.validate_host_config(&ctx).unwrap();
        }
    }
}
