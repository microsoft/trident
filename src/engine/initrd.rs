use log::info;
use std::path::Path;

use osutils::mkinitrd;
use trident_api::{error::TridentError, status::HostStatus};

use super::Module;

#[derive(Default)]
pub(super) struct InitrdModule;
impl Module for InitrdModule {
    fn name(&self) -> &'static str {
        "initrd"
    }

    fn writable_etc_overlay(&self) -> bool {
        false
    }

    #[tracing::instrument(skip_all)]
    fn configure(
        &mut self,
        _host_status: &HostStatus,
        _exec_root: &Path,
    ) -> Result<(), TridentError> {
        // We could autodetect configurations on the fly, but for more predictable
        // behavior and speedier subsequent boots, we will regenerate the host-specific initrd
        // here.

        // At the moment, this is needed for RAID, encryption, adding a root
        // password into initrd and to update the hardcoded UUID of the ESP.

        info!("Regenerating initrd");
        mkinitrd::execute()
    }
}
