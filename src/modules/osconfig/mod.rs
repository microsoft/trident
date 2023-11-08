use anyhow::Error;

use trident_api::{
    config::HostConfiguration,
    status::{HostStatus, ReconcileState, UpdateKind},
};

use crate::modules::Module;

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

    fn validate_host_config(
        &self,
        _host_status: &HostStatus,
        _host_config: &HostConfiguration,
    ) -> Result<(), Error> {
        Ok(())
    }

    fn select_update_kind(
        &self,
        _host_status: &HostStatus,
        _host_config: &HostConfiguration,
    ) -> Option<UpdateKind> {
        None
    }

    fn reconcile(
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
