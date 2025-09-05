use std::{fs, path::Path};

use log::debug;

use osutils::osmodifier::OSModifierConfig;
use trident_api::{
    config::Services,
    error::{InternalError, ReportError, ServicingError, TridentError},
};

use crate::{
    engine::{EngineContext, Subsystem},
    OS_MODIFIER_NEWROOT_PATH,
};

const SHARED_PARTITION_PATH: &str = "/var/lib/trident";

#[derive(Default)]
pub struct SysextsSubsystem;

impl Subsystem for SysextsSubsystem {
    fn name(&self) -> &'static str {
        "sysexts"
    }

    fn validate_host_config(&self, ctx: &EngineContext) -> Result<(), TridentError> {
        let Some(sysexts) = &ctx.spec.sysexts else {
            debug!("No sysexts found in HC. Returning early.");
            return Ok(());
        };
        debug!("Found the following sysexts from the HC: {sysexts:?}");

        // Ensure that all sysexts are *.raw files.
        if sysexts
            .add
            .clone()
            .into_iter()
            .any(|sysext| sysext.url.to_string().ends_with(".raw"))
        {
            return Err(TridentError::internal("Invalid sysext received"));
        };

        Ok(())
    }

    // Outside of chroot
    // fn provision(&mut self, ctx: &EngineContext, mount_path: &Path) -> Result<(), TridentError> {
    //     // Check for existing sysexts on the system

    //     Ok(())
    // }

    // Inside chroot
    fn configure(&mut self, ctx: &EngineContext) -> Result<(), TridentError> {
        let Some(sysexts) = &ctx.spec.sysexts else {
            debug!("No sysexts found in HC. Returning early.");
            return Ok(());
        };
        debug!("Found the following sysexts from the HC: {sysexts:?}");

        // Create directory for sysexts if it doesn't exist already
        fs::create_dir_all(Path::new(SHARED_PARTITION_PATH).join("extensions")).structured(
            InternalError::Internal(
                "failed to create directory for extensions in shared partition",
            ),
        )?;

        // Place sysexts in shared partition
        for sysext in &sysexts.add {
            fs::rename(
                sysext.url,
                Path::new(SHARED_PARTITION_PATH)
                    .join("extensions")
                    .join("placeholder"),
            )
            .structured(InternalError::Internal(
                "Failed to move sysext to the directory for sysexts",
            ))?;
        }

        // Symlink extensions to /var/lib/extensions

        // Call OS Modifier to enable systemd-sysext
        let os_modifier_config = OSModifierConfig {
            services: Some(Services {
                enable: ["systemd-sysext".to_string()].to_vec(),
                ..Default::default()
            }),
            ..Default::default()
        };
        os_modifier_config
            .call_os_modifier(Path::new(OS_MODIFIER_NEWROOT_PATH))
            .structured(ServicingError::RunOsModifier)?;
        Ok(())
    }
}
