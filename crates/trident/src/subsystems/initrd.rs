use log::{debug, info};

use osutils::mkinitrd;
use trident_api::{
    constants::internal_params::DRACUT_DEBUG, error::TridentError, is_default,
    status::ServicingType,
};

use crate::engine::{EngineContext, Subsystem};

#[derive(Default)]
pub struct InitrdSubsystem;
impl Subsystem for InitrdSubsystem {
    fn name(&self) -> &'static str {
        "initrd"
    }

    fn writable_etc_overlay(&self) -> bool {
        false
    }

    fn select_servicing_type(&self, ctx: &EngineContext) -> Result<ServicingType, TridentError> {
        if is_default(&ctx.spec_old) {
            return Ok(ServicingType::CleanInstall);
        } else if ctx.spec.image != ctx.spec_old.image {
            return Ok(ServicingType::AbUpdate);
        }
        Ok(ServicingType::NoActiveServicing)
    }

    #[tracing::instrument(name = "initrd_regeneration", skip_all)]
    fn configure(&mut self, ctx: &EngineContext) -> Result<(), TridentError> {
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
