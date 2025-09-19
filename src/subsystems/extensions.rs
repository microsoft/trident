use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Error};
use log::{debug, error};

use trident_api::{
    config::ExtensionType,
    error::{InternalError, ReportError, TridentError},
    status::ServicingType,
};

use crate::engine::{EngineContext, Subsystem};

const SYSEXT_DIRECTORY_PATH: &str = "/var/lib/extensions/";
const CONFEXT_DIRECTORY_PATH: &str = "/var/lib/confexts/";

#[derive(Default)]
pub struct SysextsSubsystem;

impl Subsystem for SysextsSubsystem {
    fn name(&self) -> &'static str {
        "sysexts"
    }

    // Outside of chroot
    fn provision(&mut self, ctx: &EngineContext, mount_path: &Path) -> Result<(), TridentError> {
        let sysext_dir_path = mount_path.join(SYSEXT_DIRECTORY_PATH);
        let confext_dir_path = mount_path.join(CONFEXT_DIRECTORY_PATH);

        set_up_extensions(ctx, ExtensionType::Sysext, sysext_dir_path, mount_path)
            .structured(InternalError::Internal("Failed to set up sysexts"))?;
        set_up_extensions(ctx, ExtensionType::Confext, confext_dir_path, mount_path)
            .structured(InternalError::Internal("Failed to set up confexts"))?;

        Ok(())
    }
}

fn set_up_extensions(
    ctx: &EngineContext,
    ext_type: ExtensionType,
    ext_dir_path: PathBuf,
    mount_path: &Path,
) -> Result<(), Error> {
    let curr_exts_hashmap = &ctx
        .extensions_old
        .iter()
        .filter(|ext| ext.ext_type == ext_type)
        .map(|ext| (ext.id.clone(), ext))
        .collect::<HashMap<_, _>>();
    let curr_exts_ids: HashSet<_> = curr_exts_hashmap.keys().collect();

    let new_exts_hashmap = &ctx
        .extensions
        .iter()
        .filter(|ext| ext.ext_type == ext_type)
        .map(|ext| (ext.id.clone(), ext))
        .collect::<HashMap<_, _>>();
    let new_exts_ids: HashSet<_> = new_exts_hashmap.keys().collect();

    let mut ids_to_add: Vec<_> = new_exts_ids.difference(&curr_exts_ids).collect();
    let mut ids_to_remove: Vec<_> = curr_exts_ids.difference(&new_exts_ids).collect();
    let mut ids_to_keep_as_is: Vec<&String> = Vec::new();

    for ext_id in new_exts_ids.intersection(&curr_exts_ids) {
        // Check hash
        let curr_hash = curr_exts_hashmap
            .get(*ext_id)
            .context("Failed to find extension")?
            .sha384
            .clone();
        let new_hash = new_exts_hashmap
            .get(*ext_id)
            .context("Failed to find extension")?
            .sha384
            .clone();

        if curr_hash == new_hash {
            ids_to_keep_as_is.push(ext_id);
        } else {
            ids_to_add.push(ext_id);
            ids_to_remove.push(ext_id);
        }
    }

    let extensions_to_add: Vec<_> = new_exts_hashmap
        .into_iter()
        .filter(|(k, _)| ids_to_add.contains(&k))
        .map(|(_, ext)| ext)
        .collect();
    let extensions_to_remove: Vec<_> = curr_exts_hashmap
        .into_iter()
        .filter(|(k, _)| ids_to_remove.contains(&k))
        .map(|(_, ext)| ext)
        .collect();
    let extensions_to_keep_as_is: Vec<_> = curr_exts_hashmap
        .into_iter()
        .filter(|(k, _)| ids_to_keep_as_is.contains(&k))
        .map(|(_, ext)| ext)
        .collect();

    // Remove extensions that should be removed
    for ext in extensions_to_remove {
        fs::remove_file(&ext.location).context("Failed to delete file")?;
    }

    // Add new extensions that should be added
    fs::create_dir_all(ext_dir_path).context("Failed to create sysext dir path")?;
    for ext in extensions_to_add {
        let curr_temp_location = ext
            .temp_location
            .clone()
            .context("Failed to find temporary location of extension")?;
        let new_location = mount_path.join(&ext.location);
        fs::copy(&curr_temp_location, &new_location).context(format!(
            "Failed to copy extension from {} to {}",
            curr_temp_location.display(),
            new_location.display()
        ))?;
    }

    if ctx.servicing_type != ServicingType::HotPatch {
        for ext in extensions_to_keep_as_is {
            let new_location = mount_path.join(&ext.location);
            fs::copy(&ext.location, &new_location).context(format!(
                "Failed to copy extension from {} to {}",
                &ext.location.display(),
                new_location.display()
            ))?;
        }
    }

    Ok(())
}
