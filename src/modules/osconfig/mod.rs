use std::path::Path;

use anyhow::Context;
use log::{debug, error, info, warn};

use osutils::path;
use trident_api::{
    config::{HostConfiguration, ManagementOs, Os, SshMode},
    error::{ExecutionEnvironmentMisconfigurationError, ReportError, ServicingError, TridentError},
    status::{HostStatus, ServicingType},
};

use crate::{modules::Module, OS_MODIFIER_BINARY_PATH};

mod hostname;
mod users;

/// Returns whether the given OS configuration requires the os-modifier binary to be present.
fn requires_os_modifier_os(os_config: &Os) -> bool {
    !os_config.users.is_empty() || os_config.hostname.is_some()
}

/// Returns whether the given MOS configuration requires the os-modifier binary to be present.
fn requires_os_modifier_mos(mos_config: &ManagementOs) -> bool {
    !mos_config.users.is_empty()
}

#[derive(Default, Debug)]
pub struct OsConfigModule;
impl Module for OsConfigModule {
    fn name(&self) -> &'static str {
        "os-config"
    }

    fn validate_host_config(
        &self,
        _host_status: &HostStatus,
        host_config: &HostConfiguration,
        _planned_servicing_type: ServicingType,
    ) -> Result<(), TridentError> {
        // If the os-modifier binary is required but not present, return an error.
        if requires_os_modifier_os(&host_config.os) && !Path::new(OS_MODIFIER_BINARY_PATH).exists()
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

    fn configure(
        &mut self,
        host_status: &mut HostStatus,
        exec_root: &Path,
    ) -> Result<(), TridentError> {
        // TODO: When we switch to MIC, figure out a strategy for handling
        // other kinds of updates. Limit operation to:
        // 1. ServicingType::CleanInstall,
        // 2. ServicingType::AbUpdate, to be able to do E2E A/B update testing.
        if host_status.servicing_type != Some(ServicingType::CleanInstall)
            && host_status.servicing_type != Some(ServicingType::AbUpdate)
        {
            debug!(
                "Skipping step 'Configure' for module '{}' during servicing type '{:?}'",
                self.name(),
                host_status.servicing_type
            );
            return Ok(());
        }

        // Get the path to the os-modifier binary. We've already validated that
        // it exists when required in 'validate_host_config'.
        let os_modifier_path = path::join_relative(exec_root, OS_MODIFIER_BINARY_PATH);

        if !host_status.spec.os.users.is_empty() {
            users::set_up_users(&host_status.spec.os.users, &os_modifier_path)
                .structured(ServicingError::SetUpUsers)?;
        }

        if let Some(ref hostname) = host_status.spec.os.hostname {
            hostname::set_up_hostname(hostname, &os_modifier_path)
                .structured(ServicingError::SetUpHostname)?;
        }

        Ok(())
    }
}

#[derive(Default, Debug)]
pub struct MosConfigModule;
impl Module for MosConfigModule {
    fn name(&self) -> &'static str {
        "mos-config"
    }

    fn validate_host_config(
        &self,
        _host_status: &HostStatus,
        host_config: &HostConfiguration,
        planned_servicing_type: ServicingType,
    ) -> Result<(), TridentError> {
        if planned_servicing_type != ServicingType::CleanInstall {
            debug!(
                "Skipping step 'Validate' for module '{}' during servicing type '{:?}'",
                self.name(),
                planned_servicing_type
            );
            return Ok(());
        }

        // If the os-modifier binary is required but not present, return an error.
        if requires_os_modifier_mos(&host_config.management_os)
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

    fn prepare(&mut self, host_status: &HostStatus) -> Result<(), TridentError> {
        if host_status.servicing_type != Some(ServicingType::CleanInstall) {
            debug!(
                "Skipping step 'Prepare' for module '{}' during servicing type '{:?}'",
                self.name(),
                host_status.servicing_type
            );
            return Ok(());
        }

        // Get the path to the os-modifier binary. We've already validated that
        // it exists when required in 'validate_host_config'.
        let os_modifier_path = Path::new(OS_MODIFIER_BINARY_PATH);

        if !host_status.spec.management_os.users.is_empty() {
            info!("Setting up users for management OS");
            users::set_up_users(&host_status.spec.management_os.users, os_modifier_path)
                .structured(ServicingError::SetUpUsers)?;

            // If the config enables SSH for any MOS user, then we changed the
            // SSHD config, meaning we need to restart SSHD.
            if host_status
                .spec
                .management_os
                .users
                .iter()
                .any(|u| u.ssh_mode != SshMode::Block)
            {
                // Try to restart sshd. If it fails, log the error but don't
                // break the deployment.
                warn!("Users with SSH access were added to MOS, restarting sshd.");
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
    use trident_api::config::{Password, User};

    #[test]
    fn test_requires_os_modifier_os() {
        use super::requires_os_modifier_os;
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
        };
        let mut os = mk_os();
        assert!(!requires_os_modifier_os(&os));

        os.users.push(User {
            name: "test".to_string(),
            password: Password::Locked,
            ..Default::default()
        });
        assert!(requires_os_modifier_os(&os));

        os = mk_os();
        os.hostname = Some("test".to_string());
        assert!(requires_os_modifier_os(&os));
    }

    #[test]
    fn test_requires_os_modifier_mos() {
        use super::requires_os_modifier_mos;
        use trident_api::config::ManagementOs;

        // Manually create an empty ManagementOs struct. This is the same as
        // ManagementOs::default(), but this way it will break if the struct
        // changes in the future, forcing us to update this test.
        let mut mos = ManagementOs {
            users: vec![],
            network: None,
        };
        assert!(!requires_os_modifier_mos(&mos));

        mos.users.push(User {
            name: "test".to_string(),
            password: Password::Locked,
            ..Default::default()
        });
        assert!(requires_os_modifier_mos(&mos));
    }
}
