use std::path::Path;

use trident_api::{
    error::{ManagementError, ReportError, TridentError},
    status::HostStatus,
};

use crate::modules::Module;

pub(super) mod esp;
pub(super) mod grub;

#[derive(Default, Debug)]
pub(super) struct BootModule;
impl Module for BootModule {
    fn name(&self) -> &'static str {
        "boot"
    }

    fn provision(
        &mut self,
        host_status: &mut HostStatus,
        mount_point: &Path,
    ) -> Result<(), TridentError> {
        // Perform file-based deployment of ESP images, if needed, after filesystems have been
        // mounted and initialized
        esp::deploy_esp_images(host_status, mount_point)
            .structured(ManagementError::DeployESPImages)?;

        Ok(())
    }

    fn configure(
        &mut self,
        host_status: &mut HostStatus,
        _exec_root: &Path,
    ) -> Result<(), TridentError> {
        grub::update_configs(host_status).structured(ManagementError::UpdateGrubConfigs)?;

        Ok(())
    }
}
