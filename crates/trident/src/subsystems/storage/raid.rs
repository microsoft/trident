use std::{fs, path::Path};

use anyhow::{Context, Error};
use log::{debug, trace, warn};

use osutils::{files, mdadm};

use crate::engine::EngineContext;

const MDADM_CONFIG_FILE: &str = "/etc/mdadm/mdadm.conf";

#[tracing::instrument(name = "raid_configuration", skip_all)]
pub(super) fn configure(ctx: &EngineContext) -> Result<(), Error> {
    if !ctx.spec.storage.raid.software.is_empty() {
        let output = mdadm::examine().context("Failed to examine RAID arrays")?;

        debug!("Creating mdadm config file '{}'", MDADM_CONFIG_FILE);
        files::create_file(MDADM_CONFIG_FILE).context("Failed to create mdadm config file")?;
        fs::write(Path::new(MDADM_CONFIG_FILE), &output)
            .context("Failed to write mdadm config file")?;

        trace!(
            "Contents of mdadm config file at '{}':\n{}",
            MDADM_CONFIG_FILE,
            output
        );
    }

    Ok(())
}
