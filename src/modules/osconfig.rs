use anyhow::Error;

use trident_api::{
    config::HostConfiguration,
    status::{HostStatus, UpdateKind},
};

use crate::modules::Module;

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
        _host_config: &HostConfiguration,
    ) -> Result<(), Error> {
        // TODO(5993): add user configuration
        Ok(())
    }
}
