use anyhow::Error;

use trident_api::{
    config::HostConfiguration,
    status::{HostStatus, UpdateKind},
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
        _host_status: &mut HostStatus,
        host_config: &HostConfiguration,
    ) -> Result<(), Error> {
        users::set_up_users(host_config.osconfig.users.clone())?;

        //TODO(6031): Implement changing sshd_config

        Ok(())
    }
}
