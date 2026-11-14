//! Slot-owned MSHV validation and efitreeloader (ETL) ESP definitions.

use std::path::Path;

use anyhow::{anyhow, ensure, Context, Error};

use sysdefs::arch::SystemArchitecture;
use trident_api::{
    constants::ESP_EFI_DIRECTORY,
    error::{InvalidInputError, TridentError},
    status::ServicingType,
};

use crate::{
    engine::{
        boot::{self, mshv as boot_mshv},
        EngineContext,
    },
    osimage::bootloader::Bootloader,
};

/// Filename of the second-stage EFI executable for efitreeloader (ETL).
pub(super) const ETL_SECOND_STAGE_EFI_FILENAME: &str = "second.efi";

/// Enforces support restrictions for file-based deployment of MSHV images.
pub(super) fn validate_configuration(ctx: &EngineContext) -> Result<(), TridentError> {
    if ctx.is_stream_image || !ctx.bootloader().is_some_and(Bootloader::uses_mshv) {
        return Ok(());
    }
    // The required bootloader files currently support only x86-64.
    if !SystemArchitecture::current().is_x64() || !ctx.image_distro().is_acl() || !ctx.is_uki()? {
        return Err(unsupported(
            "Slot-owned MSHV requires an x86-64 ACL UKI image",
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
    Ok(())
}

/// Rejects A/B updates that change the retained slot's MSHV layout.
pub(super) fn validate_transition(ctx: &EngineContext, esp_path: &Path) -> Result<(), Error> {
    if ctx.servicing_type != ServicingType::AbUpdate {
        return Ok(());
    }
    let active_volume = ctx
        .ab_active_volume
        .context("A/B active volume is not set")?;
    let active_bundle = esp_path
        .join(ESP_EFI_DIRECTORY)
        .join(boot::make_esp_dir_name(ctx.install_index, active_volume));
    ensure!(
        boot_mshv::has_slot_mshv_bundle(&active_bundle)?
            == ctx.bootloader().is_some_and(Bootloader::uses_mshv),
        "Changing slot-owned MSHV mode on an existing installation is not supported"
    );
    Ok(())
}

/// Reports unsupported MSHV boot configurations.
fn unsupported(reason: &str) -> TridentError {
    TridentError::with_source(
        InvalidInputError::InvalidBootConfiguration,
        anyhow!("{reason}"),
    )
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
        engine::boot::mshv::FILES,
        osimage::{bootloader::BootloaderType, mock::MockOsImage},
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
                for name in FILES {
                    let path = esp.path().join("EFI/AZL2B").join(name);
                    fs::create_dir_all(path.parent().unwrap()).unwrap();
                    fs::write(path, b"payload").unwrap();
                }
            }
            let mut ctx = EngineContext {
                servicing_type: ServicingType::AbUpdate,
                install_index: 1,
                ab_active_volume: Some(AbVolumeSelection::VolumeB),
                ..Default::default()
            };
            for requested_mshv in [false, true] {
                let bootloader_type = if requested_mshv {
                    BootloaderType::EfitreeloaderMshvSystemdBoot
                } else {
                    BootloaderType::SystemdBoot
                };
                ctx.bootloader = Some(Bootloader::new(bootloader_type, None));
                assert_eq!(
                    validate_transition(&ctx, esp.path()).is_ok(),
                    retained_mshv == requested_mshv
                );
            }
        }
    }
}
