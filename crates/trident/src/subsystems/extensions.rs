use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Error};
use chrono::Utc;
use log::{debug, trace};

use trident_api::{
    config::HostConfigurationDynamicValidationError,
    error::{InvalidInputError, ReportError, ServicingError, TridentError},
    status::ServicingType,
};

use crate::engine::{
    extensions::{ExtensionData, ExtensionType},
    EngineContext, Subsystem,
};

const SYSEXT_DIRECTORY_PATH: &str = "/var/lib/extensions/";
const CONFEXT_DIRECTORY_PATH: &str = "/var/lib/confexts/";
const PREV_EXT_STATE_DIRECTORY: &str = "/var/lib/trident-extensions";
const SNAPSHOT_DIRECTORY_PREFIX: &str = "extensions_";

#[derive(Default)]
pub struct ExtensionsSubsystem;

impl Subsystem for ExtensionsSubsystem {
    fn name(&self) -> &'static str {
        "extensions"
    }

    fn select_servicing_type(
        &self,
        _ctx: &EngineContext,
    ) -> Result<Option<ServicingType>, TridentError> {
        debug!("Servicing type required for extensions subsystem is Hot Patch");
        Ok(Some(ServicingType::HotPatch))
    }

    fn validate_host_config(&self, ctx: &EngineContext) -> Result<(), TridentError> {
        // Ensure that the filename matches the intended final location. This
        // checks the validity of any user-provided locations.
        for ext in &ctx.extensions {
            let final_file_name = ext
                .location
                .file_name()
                .structured(InvalidInputError::from(
                    HostConfigurationDynamicValidationError::InvalidExtensionImagePath {
                        path: ext.location.display().to_string(),
                        ext_type: ext.ext_type.to_string(),
                    },
                ))?;
            if final_file_name != ext.name.as_str() {
                return Err(TridentError::new(InvalidInputError::from(
                    HostConfigurationDynamicValidationError::InvalidExtensionImagePath {
                        path: ext.location.display().to_string(),
                        ext_type: ext.ext_type.to_string(),
                    },
                )));
            }
        }

        Ok(())
    }

    // Outside of chroot
    fn provision(&mut self, ctx: &EngineContext, mount_path: &Path) -> Result<(), TridentError> {
        // Skip step if no changes need to be made.
        if ctx.spec.os.extensions == ctx.spec_old.os.extensions {
            debug!(
                "Skipping step 'provision' for Extensions subsystem since there \
            are no changes to the 'extensions' section of the Host Configuration."
            );
            return Ok(());
        }

        let sysext_dir_path = mount_path.join(SYSEXT_DIRECTORY_PATH);
        let confext_dir_path = mount_path.join(CONFEXT_DIRECTORY_PATH);

        // Snapshot existing extension image files to enable rolling back to a
        // previous state.
        retain_previous_ext_state(&ctx.extensions_old)
            .structured(ServicingError::CreateExtensionImagesSnapshot)?;

        // Set up new sysexts and confexts in the appropriate directories.
        set_up_extensions(ctx, ExtensionType::Sysext, sysext_dir_path, mount_path).structured(
            ServicingError::SetUpExtensionImages {
                ext_type: "sysexts".to_string(),
            },
        )?;
        set_up_extensions(ctx, ExtensionType::Confext, confext_dir_path, mount_path).structured(
            ServicingError::SetUpExtensionImages {
                ext_type: "confexts".to_string(),
            },
        )?;

        // Clean up extension images.
        clean_up_extensions(ctx).structured(ServicingError::CleanupTemporaryExtensionImages)?;

        Ok(())
    }
}

/// Copy all current extension images from current locations to snapshot
/// directory inside /var/lib/trident-extensions.
fn retain_previous_ext_state(current_exts: &Vec<ExtensionData>) -> Result<(), Error> {
    // Current snapshot of extension images should be within a directory named after current time
    let curr_time = Utc::now().format("%Y-%m-%d_%H:%M:%S").to_string();
    let snapshot_dir = format!("{PREV_EXT_STATE_DIRECTORY}/{SNAPSHOT_DIRECTORY_PREFIX}{curr_time}");

    // Ensure that `/var/lib/trident-extensions/extensions_<time>` exists
    trace!("Creating snapshot directory at '{snapshot_dir}'");
    fs::create_dir_all(&snapshot_dir)
        .with_context(|| format!("Failed to create directory path '{snapshot_dir}'"))?;

    for ext in current_exts {
        let file_name = ext.location.file_name().with_context(|| {
            format!(
                "Failed to get file name from file location '{}'",
                ext.location.display()
            )
        })?;
        let snapshot_file_path = Path::new(&snapshot_dir).join(file_name);
        fs::copy(&ext.location, &snapshot_file_path).with_context(|| {
            format!(
                "Failed to copy extension '{}' to '{}'",
                ext.location.display(),
                snapshot_file_path.display()
            )
        })?;
    }

    Ok(())
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

    // Identify extension images that should be updated.
    for ext_id in new_exts_ids.intersection(&curr_exts_ids) {
        // Check hash
        let curr_hash = curr_exts_hashmap
            .get(*ext_id)
            .context(format!("Failed to find extension id '{ext_id}'"))? // We should never error here
            .sha384
            .clone();
        let new_hash = new_exts_hashmap
            .get(*ext_id)
            .context(format!("Failed to find extension id '{ext_id}'"))? // We should never error here
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
        .iter()
        .filter(|(k, _)| ids_to_add.contains(&k))
        .map(|(_, ext)| ext)
        .collect();
    let extensions_to_remove: Vec<_> = curr_exts_hashmap
        .iter()
        .filter(|(k, _)| ids_to_remove.contains(&k))
        .map(|(_, ext)| ext)
        .collect();
    let extensions_to_keep_as_is: Vec<_> = curr_exts_hashmap
        .iter()
        .filter(|(k, _)| ids_to_keep_as_is.contains(k))
        .map(|(_, ext)| ext)
        .collect();

    // Remove extensions that should be removed
    for ext in extensions_to_remove {
        fs::remove_file(&ext.location)
            .with_context(|| format!("Failed to delete file at '{}'", ext.location.display()))?;
    }

    // Add new extensions that should be added
    fs::create_dir_all(&ext_dir_path).with_context(|| {
        format!(
            "Failed to create extension image directory path '{}'",
            ext_dir_path.display()
        )
    })?;
    for ext in extensions_to_add {
        let curr_temp_location = ext
            .temp_location
            .clone()
            .context("Failed to find temporary location of extension image")?;
        let new_location = mount_path.join(&ext.location);
        fs::copy(&curr_temp_location, &new_location).context(format!(
            "Failed to copy extension from '{}' to '{}'",
            curr_temp_location.display(),
            new_location.display()
        ))?;
    }

    // If the servicing OS is not the same as the target OS, copy over
    // extensions images.
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

/// Deletes extensions from temporary locations.
fn clean_up_extensions(ctx: &EngineContext) -> Result<(), Error> {
    for ext in &ctx.extensions {
        if let Some(temp_location) = &ext.temp_location {
            fs::remove_file(temp_location).with_context(|| {
                format!(
                    "Failed to remove extension image from temporary path '{}'",
                    temp_location.display()
                )
            })?;
        } else {
            return Err(Error::msg(format!(
                "Failed to find temporary location of '{}' with id '{}'",
                ext.ext_type, ext.id
            )));
        }
    }
    Ok(())
}
