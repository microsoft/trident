use std::path::Path;

use anyhow::{Context, Error};

use trident_api::status::HostStatus;

use crate::modules::Module;

pub(super) mod esp;
pub(super) mod grub;

#[derive(Default, Debug)]
pub(super) struct BootModule;
impl Module for BootModule {
    fn name(&self) -> &'static str {
        "boot"
    }

    fn provision(&mut self, host_status: &mut HostStatus, mount_point: &Path) -> Result<(), Error> {
        // Perform file-based update of ESP images, if needed, after filesystems have been mounted and
        // initialized
        esp::update_esp_images(host_status, mount_point)
            .context("Failed to perform file-based update of ESP images")?;

        Ok(())
    }

    fn configure(&mut self, host_status: &mut HostStatus, _exec_root: &Path) -> Result<(), Error> {
        grub::update_configs(host_status).context("Failed to update GRUB configs")?;

        Ok(())
    }
}
