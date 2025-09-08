use std::{fs, os::unix::fs as fs_unix, path::Path};

use log::{debug, error};

use osutils::{osmodifier::OSModifierConfig, path};
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
        debug!("validate: Found the following sysexts from the HC: {sysexts:?}");

        // Ensure that all sysexts are *.raw files.
        if let Some(sysext) = sysexts
            .add
            .clone()
            .into_iter()
            .find(|sysext| !sysext.url.to_string().ends_with(".raw"))
        {
            error!("Invalid sysext: {:?}", sysext.url);
            return Err(TridentError::internal("Invalid sysext received"));
        };

        Ok(())
    }

    // Outside of chroot
    fn provision(&mut self, ctx: &EngineContext, mount_path: &Path) -> Result<(), TridentError> {
        let Some(sysexts) = &ctx.spec.sysexts else {
            debug!("No sysexts found in HC. Returning early.");
            return Ok(());
        };
        debug!("provision: Found the following sysexts from the HC: {sysexts:?}");

        // Create directory for sysexts in shared partition if it doesn't exist already
        let provisioned_os_shared_partition_path =
            path::join_relative(mount_path, SHARED_PARTITION_PATH);
        fs::create_dir_all(provisioned_os_shared_partition_path.join("extensions")).structured(
            InternalError::Internal(
                "failed to create directory for extensions in shared partition",
            ),
        )?;

        // Move the sysext files from the MOS to the ROS
        for sysext in &sysexts.add {
            let current_file_path = sysext
                .url
                .to_file_path()
                .unwrap_or_default()
                .display()
                .to_string();
            let sysext_file_name = &current_file_path.split("/").last().unwrap_or_default();
            let new_file_path = &provisioned_os_shared_partition_path
                .join("extensions")
                .join(sysext_file_name);
            debug!("Attempting to move sysext from {current_file_path} to {new_file_path:?}");
            fs::copy(&current_file_path, new_file_path).structured(InternalError::Internal(
                "Failed to move sysext to the directory for sysexts",
            ))?;
        }

        Ok(())
    }

    // Inside chroot
    fn configure(&mut self, ctx: &EngineContext) -> Result<(), TridentError> {
        let Some(sysexts) = &ctx.spec.sysexts else {
            debug!("No sysexts found in HC. Returning early.");
            return Ok(());
        };
        debug!("configure: Found the following sysexts from the HC: {sysexts:?}");

        // Create directory for sysexts in shared partition if it doesn't exist already
        fs::create_dir_all(Path::new(SHARED_PARTITION_PATH).join("extensions")).structured(
            InternalError::Internal(
                "failed to create directory for extensions in shared partition",
            ),
        )?;

        // Create directory for sysexts in /var/lib/extensions if it doesn't exist already
        debug!("Ensure /var/lib/extensions exists");
        fs::create_dir_all("/var/lib/extensions").structured(InternalError::Internal(
            "failed to create directory for extensions in newroot at /var/lib/extensions",
        ))?;

        // Place sysexts in shared partition
        for sysext in &sysexts.add {
            let current_file_path = sysext
                .url
                .to_file_path()
                .unwrap_or_default()
                .display()
                .to_string();
            let sysext_file_name = &current_file_path.split("/").last().unwrap_or_default();
            let new_file_path = Path::new(SHARED_PARTITION_PATH)
                .join("extensions")
                .join(sysext_file_name);
            debug!("Attempting to move sysext from {current_file_path} to {new_file_path:?}");

            let symlink_path = Path::new("/var/lib/extensions").join(sysext_file_name);
            debug!("Add symlink from {new_file_path:?} to {symlink_path:?}");
            fs_unix::symlink(&new_file_path, symlink_path)
                .structured(InternalError::Internal("Failed to make symlink"))?;
        }

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
