use std::{fs, path::Path};

use anyhow::{Context, Error};
use log::info;

use trident_api::{
    config::HostConfiguration,
    status::{HostStatus, ReconcileState, UpdateKind},
};

use crate::modules::Module;
use crate::OS_MODIFIER_BINARY_PATH;

mod users;

#[derive(Default, Debug)]
pub struct OsConfigModule;
impl Module for OsConfigModule {
    fn name(&self) -> &'static str {
        "os-config"
    }

    fn refresh_host_status(&mut self, _host_status: &mut HostStatus) -> Result<(), Error> {
        Ok(())
    }

    fn select_update_kind(
        &self,
        _host_status: &HostStatus,
        _host_config: &HostConfiguration,
    ) -> Option<UpdateKind> {
        None
    }

    // TODO: Revisit this to handle read-only path in runtime os
    // Also, when os modifier becomes available in RPM, install it in the provisioning and runtime OSs.
    // Tracked by https://dev.azure.com/mariner-org/ECF/_workitems/edit/6327/
    // and https://dev.azure.com/mariner-org/ECF/_workitems/edit/6303
    fn provision(
        &mut self,
        _host_status: &mut HostStatus,
        _host_config: &HostConfiguration,
        mount_path: &Path,
    ) -> Result<(), Error> {
        info!("Copying os modifier binary to runtime OS");
        fs::copy(
            OS_MODIFIER_BINARY_PATH,
            mount_path.join(&OS_MODIFIER_BINARY_PATH[1..]),
        )
        .context("Failed to copy os modifier binary to runtime OS")?;

        Ok(())
    }

    fn configure(
        &mut self,
        host_status: &mut HostStatus,
        host_config: &HostConfiguration,
    ) -> Result<(), Error> {
        // TODO: When we switch to MIC, figure out a strategy for handling
        // other kinds of updates. Limit operation to:
        // 1. ReconcileState::CleanInstall,
        // 2. ReconcileState::UpdateInProgress(UpdateKind::AbUpdate), to be
        // able to test e2e A/B update.
        if host_status.reconcile_state != ReconcileState::CleanInstall
            && host_status.reconcile_state != ReconcileState::UpdateInProgress(UpdateKind::AbUpdate)
        {
            return Ok(());
        }

        users::set_up_users(host_config.osconfig.users.clone())?;

        Ok(())
    }
}
