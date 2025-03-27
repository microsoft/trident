use anyhow::{Context, Error};
use log::debug;

use osutils::swap;
use trident_api::status::ServicingType;

use crate::engine::EngineContext;

/// Creates swap spaces on block devices as specified in the configuration.
#[tracing::instrument(name = "swap_creation", skip_all)]
pub(super) fn create_swap(ctx: &EngineContext) -> Result<(), Error> {
    // Swap devices may NOT exist on A/B volumes, we should only create them on
    // clean installs!
    if ctx.servicing_type != ServicingType::CleanInstall {
        debug!(
            "Skipping swap creation for servicing type {:?}",
            ctx.servicing_type
        );

        return Ok(());
    }

    debug!("Creating swap on block devices");

    for swap in ctx.spec.storage.swap.iter() {
        let device_path = ctx
            .get_block_device_path(&swap.device_id)
            .with_context(|| format!("Failed to get device path for '{}'", swap.device_id))?;

        debug!("Creating swap on block device {:?}", device_path);

        swap::mkswap(&device_path)
            .with_context(|| format!("Failed to create swap space on '{}'", swap.device_id))?;
    }

    Ok(())
}
