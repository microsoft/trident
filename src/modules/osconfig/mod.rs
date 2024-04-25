use std::path::Path;

use anyhow::{Context, Error};
use log::{debug, warn};

use osutils::path;
use trident_api::{
    config::HostConfiguration,
    status::{HostStatus, ReconcileState, UpdateKind},
};

use crate::{modules::Module, OS_MODIFIER_BINARY_PATH};

mod hostname;
mod users;

#[derive(Default, Debug)]
pub struct OsConfigModule;
impl Module for OsConfigModule {
    fn name(&self) -> &'static str {
        "os-config"
    }

    fn configure(
        &mut self,
        host_status: &mut HostStatus,
        host_config: &HostConfiguration,
        exec_root: &Path,
    ) -> Result<(), Error> {
        // TODO: When we switch to MIC, figure out a strategy for handling
        // other kinds of updates. Limit operation to:
        // 1. ReconcileState::CleanInstall,
        // 2. ReconcileState::UpdateInProgress(UpdateKind::AbUpdate), to be
        // able to test e2e A/B update.
        if host_status.reconcile_state != ReconcileState::CleanInstall
            && host_status.reconcile_state != ReconcileState::UpdateInProgress(UpdateKind::AbUpdate)
        {
            debug!(
                "Skipping os-config module for reconcile state: {:?}",
                host_status.reconcile_state
            );
            return Ok(());
        }

        let os_modifier_path = path::join_relative(exec_root, OS_MODIFIER_BINARY_PATH);
        if !os_modifier_path.exists() {
            warn!("os-modifier binary not found at '{OS_MODIFIER_BINARY_PATH}'");
        }

        if !host_config.os.users.is_empty() {
            users::set_up_users(&host_config.os.users, &os_modifier_path)
                .context("Failed to set up users")?;
        }

        if let Some(hostname) = host_config.os.hostname.clone() {
            hostname::set_up_hostname(&hostname, &os_modifier_path)
                .context("Failed to set up hostname")?;
        }

        Ok(())
    }
}
