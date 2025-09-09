use std::{
    collections::HashMap,
    fs, io,
    os::unix::fs as fs_unix,
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{Context, Error};
use etc_os_release::OsRelease;
use log::{debug, error};

use osutils::{dependencies::Dependency, path};
use trident_api::{
    error::{InternalError, ReportError, TridentError},
    status::{ServicingType, SysextInfo},
};

use crate::engine::{EngineContext, Subsystem};

const SHARED_PARTITION_PATH: &str = "/var/lib/trident";

#[derive(Default)]
pub struct SysextsSubsystem;

impl Subsystem for SysextsSubsystem {
    fn name(&self) -> &'static str {
        "sysexts"
    }

    fn validate_host_config(&self, ctx: &EngineContext) -> Result<(), TridentError> {
        let Some(sysexts) = &ctx.spec.sysexts else {
            debug!("No sysexts found in HC. Returning early.");
            return Ok(());
        };
        debug!("validate: Found the following sysexts from the HC: {sysexts:?}");

        // Ensure that all sysexts are *.raw files.
        if let Some(sysext) = sysexts
            .add
            .clone()
            .into_iter()
            .find(|sysext| !sysext.url.to_string().ends_with(".raw"))
        {
            error!("Invalid sysext: {:?}", sysext.url);
            return Err(TridentError::internal("Invalid sysext received"));
        };

        Ok(())
    }

    // Outside of chroot
    fn provision(&mut self, ctx: &EngineContext, mount_path: &Path) -> Result<(), TridentError> {
        let Some(sysexts) = &ctx.spec.sysexts else {
            debug!("No sysexts found in HC. Returning early.");
            return Ok(());
        };
        debug!("provision: Found the following sysexts from the HC: {sysexts:?}");

        let extensions_dir = if ctx.servicing_type == ServicingType::CleanInstall {
            // Create directory for sysexts in shared partition if it doesn't exist already
            let provisioned_os_shared_partition_path =
                path::join_relative(mount_path, SHARED_PARTITION_PATH);
            fs::create_dir_all(provisioned_os_shared_partition_path.join("extensions"))
                .structured(InternalError::Internal(
                    "failed to create directory for extensions in shared partition",
                ))?;
            provisioned_os_shared_partition_path.join("extensions")
        } else {
            // Create directory for sysexts in shared partition if it doesn't exist already
            fs::create_dir_all(Path::new(SHARED_PARTITION_PATH).join("extensions")).structured(
                InternalError::Internal(
                    "failed to create directory for extensions in shared partition",
                ),
            )?;
            Path::new(SHARED_PARTITION_PATH).join("extensions")
        };

        // Move the sysext files to the shared partition
        for sysext in &sysexts.add {
            let current_file_path = sysext
                .url
                .to_file_path()
                .unwrap_or_default()
                .display()
                .to_string();
            let sysext_file_name = &current_file_path.split("/").last().unwrap_or_default();
            let new_file_path = &extensions_dir.join(sysext_file_name);
            debug!("Attempting to move sysext from {current_file_path} to {new_file_path:?}");
            fs::copy(&current_file_path, new_file_path).structured(InternalError::Internal(
                "Failed to move sysext to the directory for sysexts",
            ))?;
        }

        Ok(())
    }

    // Inside chroot
    fn configure(&mut self, ctx: &mut EngineContext) -> Result<(), TridentError> {
        let Some(sysexts) = &ctx.spec.sysexts else {
            debug!("No sysexts found in HC. Returning early.");
            return Ok(());
        };
        debug!("configure: Found the following sysexts from the HC: {sysexts:?}");

        // Create directory for sysexts in /var/lib/extensions if it doesn't exist already
        debug!("Ensure /var/lib/extensions exists");
        fs::create_dir_all("/var/lib/extensions").structured(InternalError::Internal(
            "failed to create directory for extensions in newroot at /var/lib/extensions",
        ))?;

        let mut sysext_info_hashmap = HashMap::new();

        // Place sysexts in shared partition
        for sysext in &sysexts.add {
            let current_file_path = sysext
                .url
                .to_file_path()
                .unwrap_or_default()
                .display()
                .to_string();
            let sysext_file_name = &current_file_path.split("/").last().unwrap_or_default();
            let new_file_path = Path::new(SHARED_PARTITION_PATH)
                .join("extensions")
                .join(sysext_file_name);
            debug!("Attempting to move sysext from {current_file_path} to {new_file_path:?}");

            let sysext_info = get_sysext_info(&new_file_path).structured(
                InternalError::Internal("Failed to get extension release info"),
            )?;
            sysext_info_hashmap.insert(sysext_info.id.clone(), sysext_info.clone());
        }

        debug!(
            "Found the following in engine context for existing sysexts: {:?}",
            ctx.sysexts_old
        );
        // Add existing sysexts to the list of sysexts to activate
        for sysext in &ctx.sysexts_old {
            // Replace with new ones if they are passed in
            if !sysext_info_hashmap.contains_key(&sysext.id) {
                debug!("Updating sysext with id: {}", sysext.id);
                sysext_info_hashmap.insert(sysext.id.clone(), sysext.clone());
            }
        }

        debug!("Check for sysexts that should be removed");
        for id_to_remove in &sysexts.remove {
            if let Some(removed) = sysext_info_hashmap.remove(id_to_remove) {
                debug!("Removed sysext with id: {}", removed.id);
            }
        }

        // Update symlinks
        debug!("Adding symlinks for all sysexts now");
        for sysext in sysext_info_hashmap.values() {
            let current_location = sysext.location.clone().unwrap_or_default();
            let sysext_file_name = Path::new(&current_location)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            let symlink_path = Path::new("/var/lib/extensions").join(sysext_file_name);
            debug!("Add symlink from {current_location:?} to {symlink_path:?}");
            fs_unix::symlink(&current_location, symlink_path)
                .structured(InternalError::Internal("Failed to make symlink"))?;
        }

        // Write to ctx.sysexts
        ctx.sysexts.extend(sysext_info_hashmap.values().cloned());

        Ok(())
    }
}

fn get_sysext_info(img_path: &PathBuf) -> Result<SysextInfo, Error> {
    let mount_point = "/mnt/tmp";
    fs::create_dir_all(mount_point)
        .context(format!("Failed to create directory at '{mount_point}'"))?;
    let release_dir = Path::new(mount_point).join("usr/lib/extension-release.d/");
    let loop_device_output = Dependency::Losetup
        .cmd()
        .arg("-f")
        .arg("--show")
        .arg(img_path)
        .output_and_check()
        .with_context(|| "Failed to setup loop device")?;
    let loop_device = loop_device_output.trim();
    debug!("Created loop device: {}", loop_device);
    Dependency::Mount
        .cmd()
        .arg("-t")
        .arg("ddi")
        .arg(loop_device)
        .arg(mount_point)
        .run_and_check()
        .with_context(|| {
            format!("Failed to mount loop device '{loop_device}' at '{mount_point}'")
        })?;
    debug!("Successfully mounted loop device '{loop_device}' at '{mount_point}'");

    // Get extension release file
    let mut sysext_info = read_extension_release(release_dir)?;
    sysext_info.location = Some(img_path.to_path_buf());

    Dependency::Umount
        .cmd()
        .arg(mount_point)
        .run_and_check()
        .context("Failed to unmount")?;
    Dependency::Losetup
        .cmd()
        .arg("-d")
        .arg(loop_device)
        .run_and_check()
        .context("Failed to detach loop device")?;

    debug!("Returning extension_release: {sysext_info:?}");

    Ok(sysext_info)
}

fn read_extension_release(directory: PathBuf) -> Result<SysextInfo, Error> {
    // Get extension release file
    debug!(
        "Attempting to read from directory '{}'",
        directory.display()
    );
    let files = fs::read_dir(&directory)?
        .map(|res| res.map(|e| e.path()))
        .collect::<Result<Vec<_>, io::Error>>()?;

    let path = &files[0];
    debug!("Evaluating path: '{}'", path.display());
    // Find the file whose `SYSEXT_ID` matches `name` parameter
    let extension_release_file_content = fs::read_to_string(path).context(format!(
        "Failed to read extension-release file content from '{}'",
        &path.display()
    ))?;
    debug!("Found extension release file content:\n {extension_release_file_content}");
    let extension_release_obj = OsRelease::from_str(&extension_release_file_content)
        .with_context(|| "Failed to convert extension release file content to OsRelease object")?;

    Ok(SysextInfo {
        id: extension_release_obj
            .get_value("SYSEXT_ID")
            .map(|s| s.to_string())
            .context("Could not find ID")?,
        version: extension_release_obj
            .get_value("SYSEXT_VERSION_ID")
            .map(|s| s.to_string()),
        location: None,
    })
}
