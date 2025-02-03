use std::{fs, path::Path};

use anyhow::{Context, Error};
use log::{debug, trace, warn};

use osutils::mdadm;

use crate::engine::EngineContext;

#[tracing::instrument(name = "raid_configuration", skip_all)]
pub(super) fn configure(ctx: &EngineContext) -> Result<(), Error> {
    if !ctx.spec.storage.raid.software.is_empty() {
        let output = mdadm::examine().context("Failed to examine RAID arrays")?;
        let mdadm_config_file_path = "/etc/mdadm/mdadm.conf";
        debug!("Creating mdadm config file '{}'", mdadm_config_file_path);
        trace!("Contents:\n{}", output);
        osutils::files::create_file(mdadm_config_file_path)
            .context("Failed to create mdadm config file")?;
        fs::write(Path::new(mdadm_config_file_path), output)
            .context("Failed to write mdadm config file")?;
    }
    Ok(())
}
