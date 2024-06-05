use std::path::Path;

use anyhow::{Context, Error};
use log::{debug, warn};

use osutils::path;
use trident_api::status::{HostStatus, ServicingType};

use crate::{modules::Module, OS_MODIFIER_BINARY_PATH};

mod hostname;
mod users;

#[derive(Default, Debug)]
pub struct OsConfigModule;
impl Module for OsConfigModule {
    fn name(&self) -> &'static str {
        "os-config"
    }

    fn configure(&mut self, host_status: &mut HostStatus, exec_root: &Path) -> Result<(), Error> {
        // TODO: When we switch to MIC, figure out a strategy for handling
        // other kinds of updates. Limit operation to:
        // 1. ServicingType::CleanInstall,
        // 2. ServicingType::AbUpdate, to be able to do E2E A/B update testing.
        if host_status.servicing_type != Some(ServicingType::CleanInstall)
            && host_status.servicing_type != Some(ServicingType::AbUpdate)
        {
            debug!(
                "Skipping os-config module for servicing type: {:?}",
                host_status.servicing_type
            );
            return Ok(());
        }

        let os_modifier_path = path::join_relative(exec_root, OS_MODIFIER_BINARY_PATH);
        if !os_modifier_path.exists() {
            warn!("os-modifier binary not found at '{OS_MODIFIER_BINARY_PATH}'");
        }

        if !host_status.spec.os.users.is_empty() {
            users::set_up_users(&host_status.spec.os.users, &os_modifier_path)
                .context("Failed to set up users")?;
        }

        if let Some(ref hostname) = host_status.spec.os.hostname {
            hostname::set_up_hostname(hostname, &os_modifier_path)
                .context("Failed to set up hostname")?;
        }

        Ok(())
    }
}
