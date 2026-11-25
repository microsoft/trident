use log::debug;

use trident_api::error::{ReportError, ServicingError, TridentError};

use crate::engine::{EngineContext, Subsystem};

mod config;

#[derive(Default, Debug)]
pub(crate) struct GrubSubsystem;

impl Subsystem for GrubSubsystem {
    fn name(&self) -> &'static str {
        "grub"
    }

    #[tracing::instrument(name = "grub_configuration", skip_all)]
    fn configure(&mut self, ctx: &EngineContext) -> Result<(), TridentError> {
        if ctx.is_uki()? {
            debug!("Skipping GRUB configuration because UKI is in use");
            return Ok(());
        }

        if ctx.is_stream_image {
            debug!("Skipping GRUB configuration during stream-image");
            return Ok(());
        }

        config::update_configs(ctx).structured(ServicingError::UpdateGrubConfigs)
    }
}
