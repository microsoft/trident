use log::{debug, info};

use osutils::{dependencies::Dependency, exe::RunAndCheck, mkinitrd};
use trident_api::{
    constants::internal_params::DRACUT_DEBUG,
    error::{ReportError, ServicingError, TridentError},
};

use crate::engine::{EngineContext, Subsystem};
use std::process::Command;

#[derive(Default)]
pub struct InitrdSubsystem;
impl Subsystem for InitrdSubsystem {
    fn name(&self) -> &'static str {
        "initrd"
    }

    fn writable_etc_overlay(&self) -> bool {
        false
    }

    #[tracing::instrument(name = "initrd_regeneration", skip_all)]
    fn configure(&mut self, ctx: &EngineContext) -> Result<(), TridentError> {
        let _ = Command::new("cat")
            .arg("/etc/fstab")
            .run_and_check()
            .structured(ServicingError::RegenerateInitrd);
        _ = Command::new("ls")
            .arg("-lR")
            .arg("/dev/disk")
            .run_and_check()
            .structured(ServicingError::RegenerateInitrd);
        _ = Dependency::Lsblk
            .cmd()
            .arg("--json")
            .arg("--output-all")
            .arg("--bytes")
            .output_and_check()
            .structured(ServicingError::RegenerateInitrd);
        _ = Dependency::Blkid
            .cmd()
            .output_and_check()
            .structured(ServicingError::RegenerateInitrd);

        if ctx.is_uki()? {
            debug!("Skipping initrd regeneration because UKI is in use");
            return Ok(());
        }

        // We could autodetect configurations on the fly, but for more predictable
        // behavior and speedier subsequent boots, we will regenerate the host-specific initrd
        // here.

        // At the moment, this is needed for RAID, encryption, adding a root
        // password into initrd and to update the hardcoded UUID of the ESP.

        info!("Regenerating initrd");
        mkinitrd::execute(ctx.spec.internal_params.get_flag(DRACUT_DEBUG))
    }
}
