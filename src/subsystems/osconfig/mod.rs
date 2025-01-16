use std::{fs, path::Path};

use anyhow::Context;
use log::{debug, error, info, warn};

use osutils::{osmodifier::OSModifierConfig, path};
use trident_api::{
    config::{HostConfiguration, ManagementOs, Os, SshMode},
    error::{ExecutionEnvironmentMisconfigurationError, ReportError, ServicingError, TridentError},
    status::ServicingType,
};

use crate::{
    engine::{EngineContext, Subsystem},
    OS_MODIFIER_BINARY_PATH,
};

mod users;

/// Path to the machine-id file, as expected by SystemD.
const MACHINE_ID_PATH: &str = "/etc/machine-id";

/// Returns whether the given OS configuration requires the os-modifier binary to be present.
fn os_config_requires_os_modifier(os_config: &Os) -> bool {
    !os_config.users.is_empty()
        || os_config.hostname.is_some()
        || !os_config.modules.is_empty()
        || !os_config.services.enable.is_empty()
        || !os_config.services.disable.is_empty()
        || !os_config.kernel_command_line.extra_command_line.is_empty()
}

/// Returns whether the given MOS configuration requires the os-modifier binary to be present.
fn mos_config_requires_os_modifier(mos_config: &ManagementOs) -> bool {
    !mos_config.users.is_empty()
}

#[derive(Default, Debug)]
pub struct OsConfigSubsystem;
impl Subsystem for OsConfigSubsystem {
    fn name(&self) -> &'static str {
        "os-config"
    }

    fn validate_host_config(
        &self,
        _ctx: &EngineContext,
        host_config: &HostConfiguration,
    ) -> Result<(), TridentError> {
        // If the os-modifier binary is required but not present, return an error.
        if os_config_requires_os_modifier(&host_config.os)
            && !Path::new(OS_MODIFIER_BINARY_PATH).exists()
        {
            return Err(TridentError::new(
                ExecutionEnvironmentMisconfigurationError::FindOSModifierBinary {
                    binary_path: OS_MODIFIER_BINARY_PATH.to_string(),
                    config: self.name().to_string(),
                },
            ));
        }

        Ok(())
    }

    #[tracing::instrument(name = "osconfig_provision", skip_all)]
    fn provision(&mut self, ctx: &EngineContext, mount_path: &Path) -> Result<(), TridentError> {
        if ctx.servicing_type == ServicingType::AbUpdate {
            // Copy the current machine-id to the target root mount point to
            // preserve machine identity across servicing.
            let dest_machine_id_path = path::join_relative(mount_path, MACHINE_ID_PATH);
            fs::copy(MACHINE_ID_PATH, dest_machine_id_path)
                .structured(ServicingError::CopyMachineId)?;
        }

        Ok(())
    }

    #[tracing::instrument(name = "osconfig_configuration", skip_all)]
    fn configure(&mut self, ctx: &EngineContext, exec_root: &Path) -> Result<(), TridentError> {
        // TODO: When we switch to MIC, figure out a strategy for handling
        // other kinds of updates. Limit operation to:
        // 1. ServicingType::CleanInstall,
        // 2. ServicingType::AbUpdate, to be able to do E2E A/B update testing.
        if ctx.servicing_type != ServicingType::CleanInstall
            && ctx.servicing_type != ServicingType::AbUpdate
        {
            debug!(
                "Skipping step 'Configure' for subsystem '{}' during servicing type '{:?}'",
                self.name(),
                ctx.servicing_type
            );
            return Ok(());
        }

        if !os_config_requires_os_modifier(&ctx.spec.os) {
            debug!(
                "Skipping step 'Configure' for subsystem '{}' as OS modifier is not required",
                self.name()
            );
            return Ok(());
        }

        let mut os_modifier_config = OSModifierConfig::default();

        if !ctx.spec.os.users.is_empty() {
            debug!("Setting up users");
            os_modifier_config.users =
                users::set_up_users(&ctx.spec.os.users).structured(ServicingError::SetUpUsers)?;
        }

        if ctx.spec.os.hostname.is_some() {
            debug!("Setting up hostname");
            os_modifier_config.hostname = ctx.spec.os.hostname.clone();
        }

        if !ctx.spec.os.modules.is_empty() {
            debug!("Setting up kernel modules");
            os_modifier_config.modules = ctx.spec.os.modules.to_vec();
        }

        if !ctx.spec.os.services.enable.is_empty() || !ctx.spec.os.services.disable.is_empty() {
            debug!("Setting up services");
            os_modifier_config.services = Some(ctx.spec.os.services.clone());
        }

        if !ctx
            .spec
            .os
            .kernel_command_line
            .extra_command_line
            .is_empty()
        {
            debug!(
                "Setting up kernel command line: [{}]",
                ctx.spec
                    .os
                    .kernel_command_line
                    .extra_command_line
                    .join(", ")
            );
            os_modifier_config.kernel_command_line = Some(ctx.spec.os.kernel_command_line.clone());
        }

        // Get the path to the os-modifier binary. We've already validated that
        // it exists when required in 'validate_host_config'.
        let os_modifier_path = path::join_relative(exec_root, OS_MODIFIER_BINARY_PATH);

        os_modifier_config
            .call_os_modifier(&os_modifier_path)
            .structured(ServicingError::RunOsModifier)?;

        Ok(())
    }
}

#[derive(Default, Debug)]
pub struct MosConfigSubsystem;
impl Subsystem for MosConfigSubsystem {
    fn name(&self) -> &'static str {
        "mos-config"
    }

    fn validate_host_config(
        &self,
        ctx: &EngineContext,
        host_config: &HostConfiguration,
    ) -> Result<(), TridentError> {
        if ctx.servicing_type != ServicingType::CleanInstall {
            debug!(
                "Skipping step 'Validate' for subsystem '{}' during servicing type '{:?}'",
                self.name(),
                ctx.servicing_type
            );
            return Ok(());
        }

        // If the os-modifier binary is required but not present, return an error.
        if mos_config_requires_os_modifier(&host_config.management_os)
            && !Path::new(OS_MODIFIER_BINARY_PATH).exists()
        {
            return Err(TridentError::new(
                ExecutionEnvironmentMisconfigurationError::FindOSModifierBinary {
                    binary_path: OS_MODIFIER_BINARY_PATH.to_string(),
                    config: self.name().to_string(),
                },
            ));
        }

        Ok(())
    }

    fn prepare(&mut self, ctx: &EngineContext) -> Result<(), TridentError> {
        if ctx.servicing_type != ServicingType::CleanInstall {
            debug!(
                "Skipping step 'Prepare' for subsystem '{}' during servicing type '{:?}'",
                self.name(),
                ctx.servicing_type
            );
            return Ok(());
        }

        // Get the path to the os-modifier binary. We've already validated that
        // it exists when required in 'validate_host_config'.
        let os_modifier_path = Path::new(OS_MODIFIER_BINARY_PATH);

        if !ctx.spec.management_os.users.is_empty() {
            info!("Setting up users for management OS");
            let os_modifier_config = OSModifierConfig {
                users: users::set_up_users(&ctx.spec.management_os.users)
                    .structured(ServicingError::SetUpUsers)?,
                ..Default::default()
            };
            os_modifier_config
                .call_os_modifier(os_modifier_path)
                .structured(ServicingError::RunOsModifier)?;

            // If the config enables SSH for any MOS user, then we changed the
            // SSHD config, meaning we need to restart SSHD.
            if ctx
                .spec
                .management_os
                .users
                .iter()
                .any(|u| u.ssh_mode != SshMode::Block)
            {
                // Try to restart sshd. If it fails, log the error but don't
                // break the deployment.
                debug!("Users with SSH access were added to MOS, restarting sshd.");
                if let Err(err) =
                    osutils::systemd::restart_unit("sshd").context("Failed to restart sshd in MOS")
                {
                    error!("{err:?}");
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use trident_api::config::{KernelCommandLine, Module, Password, Services, User};

    #[test]
    fn test_os_config_requires_os_modifier() {
        use super::os_config_requires_os_modifier;
        use trident_api::config::{Os, Selinux};

        // Manually create an empty Os struct. This is the same as
        // Os::default(), but this way it will break if the struct changes in
        // the future, forcing us to update this test.
        let mk_os = || Os {
            network: None,
            selinux: Selinux::default(),
            users: vec![],
            additional_files: vec![],
            hostname: None,
            ..Default::default()
        };
        let mut os = mk_os();
        assert!(!os_config_requires_os_modifier(&os));

        os.users.push(User {
            name: "test".to_string(),
            password: Password::Locked,
            ..Default::default()
        });
        assert!(os_config_requires_os_modifier(&os));

        os = mk_os();
        os.hostname = Some("test".to_string());
        assert!(os_config_requires_os_modifier(&os));

        os = mk_os();
        os.modules.push(Module {
            name: "test".to_string(),
            ..Default::default()
        });
        assert!(os_config_requires_os_modifier(&os));

        os = mk_os();
        os.services = Services {
            enable: vec!["enabled-test".to_string()],
            disable: vec!["disabled-test".to_string()],
        };
        assert!(os_config_requires_os_modifier(&os));

        os = mk_os();
        os.kernel_command_line = KernelCommandLine {
            extra_command_line: vec!["test".to_string()],
        };
        assert!(os_config_requires_os_modifier(&os));
    }

    #[test]
    fn test_mos_config_requires_os_modifier() {
        use super::mos_config_requires_os_modifier;
        use trident_api::config::ManagementOs;

        // Manually create an empty ManagementOs struct. This is the same as
        // ManagementOs::default(), but this way it will break if the struct
        // changes in the future, forcing us to update this test.
        let mut mos = ManagementOs {
            users: vec![],
            network: None,
        };
        assert!(!mos_config_requires_os_modifier(&mos));

        mos.users.push(User {
            name: "test".to_string(),
            password: Password::Locked,
            ..Default::default()
        });
        assert!(mos_config_requires_os_modifier(&mos));
    }
}
