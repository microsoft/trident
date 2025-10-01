use std::{
    fs, io,
    path::{Path, PathBuf},
    str::FromStr,
    time::Duration,
};

use anyhow::{ensure, Context, Error};
use etc_os_release::OsRelease;
use log::debug;
use osutils::dependencies::Dependency;
use tempfile::NamedTempFile;

use trident_api::{
    config::Extension,
    error::{InternalError, ReportError, TridentError},
    primitives::hash::Sha384Hash,
};
use url::Url;

use crate::{
    engine::EngineContext,
    io_utils::{
        file_reader::FileReader, hashing_reader::HashingReader384, image_streamer::stream_and_hash,
    },
};

const SYSEXT_EXTENSION_RELEASE_DIRECTORY: &str = "usr/lib/extension-release.d/";
const CONFEXT_EXTENSION_RELEASE_DIRECTORY: &str = "etc/extension-release.d/";

#[derive(Clone)]
pub struct ExtensionData {
    pub id: String,
    pub name: String,
    pub url: Url,
    pub sha384: Sha384Hash,
    pub location: PathBuf,
    pub temp_location: Option<PathBuf>,
    pub ext_type: ExtensionType,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExtensionType {
    Sysext,
    Confext,
}

impl EngineContext {
    /// Populate the `extensions` and `extensions_old` fields in EngineContext.
    pub fn populate_extensions(&mut self) -> Result<(), TridentError> {
        populate_extensions_inner(&self.spec.os.extensions, &mut self.extensions)
            .structured(InternalError::Internal("Failed to populate ctx.extensions"))?;
        populate_extensions_inner(&self.spec_old.os.extensions, &mut self.extensions_old)
            .structured(InternalError::Internal(
                "Failed to populate ctx.extensions_old",
            ))?;
        Ok(())
    }
}

fn populate_extensions_inner(
    hc_extensions: &Vec<Extension>,
    ctx_extensions: &mut Vec<ExtensionData>,
) -> Result<(), Error> {
    let temp_mp = tempfile::tempdir().context("Failed to create temporary directory")?;
    for ext in hc_extensions {
        let extension_file = match &ext.location {
            Some(extension_file) => extension_file.clone(),
            None => {
                // Persist the temporary file and get its path
                NamedTempFile::new()
                    .context("Failed to create temp file")?
                    .into_temp_path()
                    .keep()
                    .context("Failed to persist temporary extension file")?
            }
        };

        let reader = FileReader::new(&ext.url, Duration::from_secs(10))
            .context("Failed to create reader")?
            .complete_reader()
            .context("Failed to obtain complete reader")?;
        let hash_reader = HashingReader384::new(reader);
        let hash =
            stream_and_hash(hash_reader, &extension_file).context("Failed to read and write")?;
        if ext.sha384 != hash {
            return Err(Error::msg("Hashes didn't match"));
        };

        // Attach a device and mount the extension
        let device_path =
            attach_device_and_mount(&extension_file, temp_mp.path()).context("Failed to mount")?;

        // Get extension release file
        let ext_data = read_extension_release(temp_mp.path(), &extension_file, ext)
            .context("Failed to get extension release information")?;

        ctx_extensions.push(ext_data);

        // Clean-Up: unmount and detach the device
        detach_device_and_unmount(device_path, temp_mp.path()).context("Failed to unmount")?;
    }
    Ok(())
}

/// Helper function to extract information from extension-release file
fn read_extension_release(
    mount_point: &Path,
    curr_location: &Path,
    ext: &Extension,
) -> Result<ExtensionData, Error> {
    let mut prefix = "SYSEXT_";
    let sysext_release_dir = mount_point.join(SYSEXT_EXTENSION_RELEASE_DIRECTORY);
    let confext_release_dir = mount_point.join(CONFEXT_EXTENSION_RELEASE_DIRECTORY);

    // Get extension release file
    let dir = match fs::read_dir(&sysext_release_dir) {
        Ok(dir) => dir,
        Err(_) => match fs::read_dir(&confext_release_dir) {
            Ok(dir) => {
                prefix = "CONFEXT_";
                dir
            }
            Err(_) => {
                return Err(Error::msg(
                    "Failed to find extension release file for extension image.",
                ))
            }
        },
    }
    .map(|res| res.map(|e| e.path()))
    .collect::<Result<Vec<_>, io::Error>>()?;

    ensure!(
        dir.len() == 1,
        "Expected each extension image to have exactly 1 extension-release file."
    );

    let path = &dir[0];
    debug!("Evaluating path: '{}'", path.display());

    // Find the file whose `SYSEXT_ID` matches `name` parameter
    let extension_release_file_content = fs::read_to_string(path).context(format!(
        "Failed to read extension-release file content from '{}'",
        &path.display()
    ))?;
    debug!("Found extension release file content:\n {extension_release_file_content}");
    let extension_release_obj = OsRelease::from_str(&extension_release_file_content)
        .with_context(|| "Failed to convert extension release file content to OsRelease object")?;

    let extension_id = extension_release_obj
        .get_value(&format!("{prefix}ID"))
        .map(|s| s.to_string())
        .ok_or_else(|| Error::msg(format!("Could not find {prefix}ID in extension release")))?;
    let file_name = path
        .display()
        .to_string()
        .split("extension-release.")
        .last()
        .ok_or_else(|| Error::msg("Failed to get extension-release ending"))?
        .to_string();
    let location = match &ext.location {
        Some(location) => location.clone(),
        None => {
            if prefix == "SYSEXT_" {
                PathBuf::from("/var/lib/extensions").join(format!("{file_name}.raw"))
            } else {
                PathBuf::from("/var/lib/confexts").join(format!("{file_name}.raw"))
            }
        }
    };

    Ok(ExtensionData {
        id: extension_id,
        name: file_name.clone(),
        url: ext.url.clone(),
        sha384: ext.sha384.clone(),
        location,
        temp_location: Some(curr_location.to_path_buf()),
        ext_type: if prefix == "SYSEXT_" {
            ExtensionType::Sysext
        } else {
            ExtensionType::Confext
        },
    })
}

/// Helper function to mount the extension image.
fn attach_device_and_mount(image_file_path: &Path, mount_path: &Path) -> Result<String, Error> {
    let loop_device_output = Dependency::Losetup
        .cmd()
        .arg("-f")
        .arg("--show")
        .arg(image_file_path)
        .output_and_check()
        .context("Failed to attach loop device")?;
    let loop_device = loop_device_output.trim();
    Dependency::Mount
        .cmd()
        .arg("-t")
        .arg("ddi")
        .arg(loop_device)
        .arg(mount_path)
        .run_and_check()
        .context("Failed to mount")?;

    Ok(loop_device.to_string())
}

/// Helper function to unmount the extension image.
fn detach_device_and_unmount(device_path: String, mount_path: &Path) -> Result<(), Error> {
    Dependency::Umount
        .cmd()
        .arg(mount_path)
        .run_and_check()
        .context("Failed to umount")?;
    Dependency::Losetup
        .cmd()
        .arg("-d")
        .arg(device_path)
        .run_and_check()
        .context("Failed to detach loop device")?;
    Ok(())
}
