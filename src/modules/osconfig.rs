use anyhow::{Context, Error};

use trident_api::{
    config::HostConfiguration,
    status::{HostStatus, UpdateKind},
};

use crate::{modules::Module, mount};

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
        // TODO: user creation should not be hardcoded.
        mount::run_script(
            r#"sudo sh -c 'echo root:password | chpasswd'
            useradd -p $(openssl passwd -1 password) -s /bin/bash -d /home/mariner_user/ -m -G sudo mariner_user"#
        ).context("Failed to apply system config")?;

        Ok(())
    }
}
