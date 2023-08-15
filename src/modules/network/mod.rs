use crate::{
    config::HostConfig,
    modules::Module,
    status::{HostStatus, UpdateKind},
};
use anyhow::Error;

mod netplan;
pub mod provisioning;

#[derive(Default, Debug)]
pub struct NetworkModule;
impl Module for NetworkModule {
    fn name(&self) -> &'static str {
        "network"
    }

    fn refresh_host_status(&mut self, _host_status: &mut HostStatus) -> Result<(), Error> {
        Ok(())
    }

    fn validate_host_config(
        &self,
        _host_status: &HostStatus,
        _host_config: &HostConfig,
    ) -> Result<(), Error> {
        Ok(())
    }

    fn select_update_kind(
        &self,
        _host_status: &HostStatus,
        _host_config: &HostConfig,
    ) -> Option<UpdateKind> {
        Some(UpdateKind::HotPatch)
    }

    fn reconcile(
        &mut self,
        _host_status: &mut HostStatus,
        _host_config: &HostConfig,
    ) -> Result<(), Error> {
        Ok(())
    }
}
