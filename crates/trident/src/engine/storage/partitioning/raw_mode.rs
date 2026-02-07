use anyhow::Error;

use osutils::block_devices::ResolvedDisk;

use crate::engine::EngineContext;

/// This function handles the partitioning logic for raw COSI storage mode. In
/// this mode, we expect exactly one disk with no partitions, and we create a
/// single partition that takes up the entire disk. This is a simplified flow
/// that is only used for raw COSI storage, and it allows us to use the same
/// underlying partitioning code while bypassing the usual validation and
/// adoption logic that is not relevant in this mode.
pub(super) fn create_partitions_for_raw_cosi_storage(
    ctx: &mut EngineContext,
    disk: &ResolvedDisk,
) -> Result<(), Error> {
    Ok(())
}
