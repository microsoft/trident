use std::{fs, path::Path};

use anyhow::Context;
use log::{debug, error, info, warn};

use osutils::{osmodifier::OSModifierConfig, path};
use trident_api::{
    config::{ManagementOs, SshMode},
    constants::internal_params::DISABLE_HOSTNAME_CARRY_OVER,
    error::{ExecutionEnvironmentMisconfigurationError, ReportError, ServicingError, TridentError},
    status::ServicingType,
};

use crate::{
    engine::{EngineContext, Subsystem},
    OS_MODIFIER_BINARY_PATH, OS_MODIFIER_NEWROOT_PATH,
};

mod users;

/// Path to the machine-id file, as expected by SystemD.
const MACHINE_ID_PATH: &str = "/etc/machine-id";

/// Path to hostname.
const HOSTNAME_PATH: &str = "/etc/hostname";

/// Returns whether the given OS configuration requires the os-modifier binary to be present.
fn os_config_requires_os_modifier(ctx: &EngineContext) -> bool {
    let os_config = &ctx.spec.os;
    !os_config.users.is_empty()
        || os_config.hostname.is_some()
        || !os_config.modules.is_empty()
        || !os_config.services.enable.is_empty()
        || !os_config.services.disable.is_empty()
        || !os_config.kernel_command_line.extra_command_line.is_empty()
        || should_carry_over_hostname(ctx)
}

/// Returns whether the given MOS configuration requires the os-modifier binary to be present.
fn mos_config_requires_os_modifier(mos_config: &ManagementOs) -> bool {
    !mos_config.users.is_empty()
}

/// Returns whether the hostname should be updated during A/B Update.
///
/// If the OS configuration does not specify a hostname, so long as DISABLE_HOSTNAME_CARRY_OVER flag
/// is not set to true, we want to carry over the existing machine hostname after updating.
fn should_carry_over_hostname(ctx: &EngineContext) -> bool {
    !ctx.spec
        .internal_params
        .get_flag(DISABLE_HOSTNAME_CARRY_OVER)
        && ctx.servicing_type == ServicingType::AbUpdate
}

#[derive(Default, Debug)]
pub struct OsConfigSubsystem {
    prev_hostname: Option<String>,
}
impl Subsystem for OsConfigSubsystem {
    fn name(&self) -> &'static str {
        "os-config"
    }

    fn validate_host_config(&self, ctx: &EngineContext) -> Result<(), TridentError> {
        // If the os-modifier binary is required but not present, return an error.
        if os_config_requires_os_modifier(ctx) && !Path::new(OS_MODIFIER_BINARY_PATH).exists() {
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

            // Save the current hostname to carry forward into the updated volume.
            if should_carry_over_hostname(ctx) {
                self.prev_hostname = Some(
                    fs::read_to_string(HOSTNAME_PATH)
                        .structured(ServicingError::ReadHostname {
                            path: HOSTNAME_PATH.into(),
                        })?
                        .trim()
                        .to_string(),
                );
            }
        }

        Ok(())
    }

    #[tracing::instrument(name = "osconfig_configuration", skip_all)]
    fn configure(&mut self, ctx: &EngineContext) -> Result<(), TridentError> {
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

        if !os_config_requires_os_modifier(ctx) {
            debug!(
                "Skipping step 'Configure' for subsystem '{}' as OS modifier is not required",
                self.name()
            );
            return Ok(());
        } else if ctx.is_uki()? && ctx.storage_graph.root_fs_is_verity() {
            error!("Skipping OS configuration changes requested in Host Configuration because UKI root verity is in use.");
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
        } else if should_carry_over_hostname(ctx) {
            // If no hostname is provided during A/B Update, carry forward the existing machine
            // hostname into the new root
            debug!("Carrying over hostname");
            os_modifier_config.hostname = self.prev_hostname.clone();
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

        // If we have a UKI image, update SELinux mode here since it cannot be set via kernel
        // command line.
        if ctx.is_uki()? && ctx.spec.os.selinux.mode.is_some() {
            debug!("Updating SELinux config");
            os_modifier_config.selinux = Some(ctx.spec.os.selinux.clone());
        }

        os_modifier_config
            .call_os_modifier(Path::new(OS_MODIFIER_NEWROOT_PATH))
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

    fn validate_host_config(&self, ctx: &EngineContext) -> Result<(), TridentError> {
        if ctx.servicing_type != ServicingType::CleanInstall {
            debug!(
                "Skipping step 'Validate' for subsystem '{}' during servicing type '{:?}'",
                self.name(),
                ctx.servicing_type
            );
            return Ok(());
        }

        // If the os-modifier binary is required but not present, return an error.
        if mos_config_requires_os_modifier(&ctx.spec.management_os)
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
    use trident_api::{
        config::{
            HostConfiguration, KernelCommandLine, ManagementOs, Module, Os, Password, Selinux,
            Services, User,
        },
        status::ServicingType,
    };

    use crate::engine::EngineContext;

    #[test]
    fn test_os_config_requires_os_modifier() {
        use super::os_config_requires_os_modifier;

        // Manually create an empty EngineContext struct. This is the same as
        // EngineContext::default(), but this way it will break if the struct
        // changes in the future, forcing us to update this test.
        let mk_ctx = || EngineContext {
            spec: HostConfiguration {
                os: Os {
                    netplan: None,
                    selinux: Selinux::default(),
                    users: vec![],
                    additional_files: vec![],
                    hostname: None,
                    modules: vec![],
                    services: Services::default(),
                    kernel_command_line: KernelCommandLine::default(),
                },
                ..Default::default()
            },
            ..Default::default()
        };
        let mut ctx = mk_ctx();
        assert!(!os_config_requires_os_modifier(&ctx));

        ctx.spec.os.users.push(User {
            name: "test".to_string(),
            password: Password::Locked,
            ..Default::default()
        });
        assert!(os_config_requires_os_modifier(&ctx));

        ctx = mk_ctx();
        ctx.spec.os.hostname = Some("test".to_string());
        assert!(os_config_requires_os_modifier(&ctx));

        ctx = mk_ctx();
        ctx.spec.os.modules.push(Module {
            name: "test".to_string(),
            ..Default::default()
        });
        assert!(os_config_requires_os_modifier(&ctx));

        ctx = mk_ctx();
        ctx.spec.os.services = Services {
            enable: vec!["enabled-test".to_string()],
            disable: vec!["disabled-test".to_string()],
        };
        assert!(os_config_requires_os_modifier(&ctx));

        ctx = mk_ctx();
        ctx.spec.os.kernel_command_line = KernelCommandLine {
            extra_command_line: vec!["test".to_string()],
        };
        assert!(os_config_requires_os_modifier(&ctx));

        ctx = mk_ctx();
        ctx.servicing_type = ServicingType::AbUpdate;
        assert!(os_config_requires_os_modifier(&ctx));
        ctx.spec.internal_params = serde_yaml::from_str("disableHostnameCarryOver: true").unwrap();
        assert!(!os_config_requires_os_modifier(&ctx));
    }

    #[test]
    fn test_mos_config_requires_os_modifier() {
        use super::mos_config_requires_os_modifier;

        // Manually create an empty ManagementOs struct. This is the same as
        // ManagementOs::default(), but this way it will break if the struct
        // changes in the future, forcing us to update this test.
        let mut mos = ManagementOs {
            users: vec![],
            netplan: None,
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

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports))]
mod functional_test {
    use super::*;

    use pytest_gen::functional_test;
    use sys_mount::{MountBuilder, MountFlags, Unmount, UnmountFlags};
    use trident_api::config::{HostConfiguration, Os};

    #[functional_test(feature = "helpers")]
    fn test_os_config_configure_hostname_clean_install() {
        // Get current system hostname
        let prev_hostname = fs::read_to_string(HOSTNAME_PATH)
            .unwrap()
            .trim()
            .to_string();

        // Create EngineContext
        let ctx = EngineContext {
            servicing_type: ServicingType::CleanInstall,
            spec: HostConfiguration {
                os: Os {
                    hostname: Some("custom-hostname".into()),
                    ..Default::default()
                },
                ..Default::default()
            },
            is_uki: Some(false),
            ..Default::default()
        };
        assert!(os_config_requires_os_modifier(&ctx));

        fs::write(OS_MODIFIER_NEWROOT_PATH, b"").unwrap();
        let _mount = MountBuilder::default()
            .flags(MountFlags::BIND)
            .mount(OS_MODIFIER_BINARY_PATH, OS_MODIFIER_NEWROOT_PATH)
            .unwrap()
            .into_unmount_drop(UnmountFlags::empty());

        // Configure OsConfig subsystem
        let mut os_config_subsystem = OsConfigSubsystem::default();
        let _ = os_config_subsystem.configure(&ctx);

        // Check that hostname has updated
        assert_eq!(
            fs::read_to_string(Path::new("/etc/hostname")).unwrap(),
            "custom-hostname"
        );

        // Clean up and revert to previous hostname
        fs::write(HOSTNAME_PATH, prev_hostname.clone()).unwrap();
        assert_eq!(
            fs::read_to_string(Path::new("/etc/hostname")).unwrap(),
            prev_hostname
        );
    }

    #[functional_test(feature = "helpers")]
    fn test_os_config_configure_hostname_ab_update() {
        // Get current system hostname
        let prev_hostname = fs::read_to_string(HOSTNAME_PATH)
            .unwrap()
            .trim()
            .to_string();

        // Create EngineContext with no hostname specified
        let ctx = EngineContext {
            servicing_type: ServicingType::AbUpdate,
            is_uki: Some(false),
            ..Default::default()
        };
        assert!(os_config_requires_os_modifier(&ctx));

        fs::write(OS_MODIFIER_NEWROOT_PATH, b"").unwrap();
        let _mount = MountBuilder::default()
            .flags(MountFlags::BIND)
            .mount(OS_MODIFIER_BINARY_PATH, OS_MODIFIER_NEWROOT_PATH)
            .unwrap()
            .into_unmount_drop(UnmountFlags::empty());

        // Configure OsConfig subsystem and set prev_hostname parameter
        let mut os_config_subsystem = OsConfigSubsystem {
            prev_hostname: Some("carry-over-hostname".into()),
        };
        let _ = os_config_subsystem.configure(&ctx);

        // Check that hostname has updated
        assert_eq!(
            fs::read_to_string(Path::new("/etc/hostname")).unwrap(),
            "carry-over-hostname"
        );

        // Clean up and revert to previous hostname
        fs::write(HOSTNAME_PATH, prev_hostname.clone()).unwrap();
        assert_eq!(
            fs::read_to_string(Path::new("/etc/hostname")).unwrap(),
            prev_hostname
        );
    }
}
