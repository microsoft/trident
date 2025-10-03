use std::{
    fmt::Display,
    fs, io,
    path::{Path, PathBuf},
    str::FromStr,
    time::Duration,
};

use anyhow::{bail, ensure, Context, Error};
use etc_os_release::OsRelease;
use log::{debug, trace};
use osutils::dependencies::Dependency;
use tempfile::NamedTempFile;

use trident_api::{
    config::Extension,
    constants::internal_params::COSI_HTTP_CONNECTION_TIMEOUT_SECONDS,
    error::{InternalError, ReportError, TridentError},
    primitives::hash::Sha384Hash,
};

use crate::{
    engine::EngineContext,
    io_utils::{
        file_reader::FileReader, hashing_reader::HashingReader384, image_streamer::stream_and_hash,
    },
};

const SYSEXT_EXTENSION_RELEASE_DIRECTORY: &str = "usr/lib/extension-release.d/";
const CONFEXT_EXTENSION_RELEASE_DIRECTORY: &str = "etc/extension-release.d/";
const SYSEXT_PREFIX: &str = "SYSEXT_";
const CONFEXT_PREFIX: &str = "CONFEXT_";

#[derive(Clone)]
pub struct ExtensionData {
    pub id: String,
    pub name: String,
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

impl Display for ExtensionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sysext => write!(f, "sysext"),
            Self::Confext => write!(f, "confext"),
        }
    }
}

impl EngineContext {
    /// Populate the `extensions` and `extensions_old` fields in EngineContext.
    pub fn populate_extensions(&mut self) -> Result<(), TridentError> {
        // No need to populate extensions object if the extensions in the Host
        // Configuration have not changed.
        if self.spec.os.extensions == self.spec_old.os.extensions {
            debug!(
                "Skipping running 'populate_extensions' step since there are \
            no changes to the 'extensions' section of the Host Configuration."
            );
            return Ok(());
        }

        let timeout = match self
            .spec
            .internal_params
            .get_u64(COSI_HTTP_CONNECTION_TIMEOUT_SECONDS)
        {
            Some(Ok(timeout)) => Duration::from_secs(timeout),
            _ => Duration::from_secs(10), // Default timeout
        };

        populate_extensions_inner(
            &self.spec.os.extensions,
            &mut self.extensions,
            timeout,
            true,
        )
        .structured(InternalError::PopulateExtensionImages(
            "Failed with new extension images.".to_string(),
        ))?;
        populate_extensions_inner(
            &self.spec_old.os.extensions,
            &mut self.extensions_old,
            timeout,
            false,
        )
        .structured(InternalError::PopulateExtensionImages(
            "Failed with existing extension images.".to_string(),
        ))?;
        Ok(())
    }

    /// Update the Host Configuration with the final location of the extension
    /// images.
    pub fn finalize_extension_locations(&mut self) -> Result<(), TridentError> {
        for ext_data in &self.extensions {
            // Find the matching extension in the Host Configuration
            let ext = self
                .spec
                .os
                .extensions
                .iter_mut()
                .find(|ext| ext.sha384 == ext_data.sha384)
                .structured(InternalError::UpdateExtensionsLocation {
                    id: ext_data.id.clone(),
                    hash: ext_data.sha384.to_string(),
                })?;
            ext.location = Some(ext_data.location.clone());
        }
        Ok(())
    }
}

fn populate_extensions_inner(
    hc_extensions: &Vec<Extension>,
    ctx_extensions: &mut Vec<ExtensionData>,
    timeout: Duration,
    new: bool,
) -> Result<(), Error> {
    let temp_mp = tempfile::tempdir()?;

    for ext in hc_extensions {
        let extension_file = if new {
            // Create and persist a temporary file; get its path
            let temp_file = NamedTempFile::new()
                .context("Failed to create temporary file")?
                .into_temp_path()
                .keep()
                .context("Failed to persist temporary file")?;

            // Download the extension image to this temporary file
            let reader = FileReader::new(&ext.url, timeout)
                .context("Failed to create file reader")?
                .complete_reader()
                .context("Failed to create complete file reader")?;
            let hash_reader = HashingReader384::new(reader);
            let computed_sha384 =
                stream_and_hash(hash_reader, &temp_file).context("Failed to read and write")?;

            // Ensure computed SHA384 matches SHA384 in Host Configuration
            if ext.sha384 != computed_sha384 {
                bail!(
                    "SHA384 mismatch for extension image at '{}': expected {}, got {}",
                    ext.url,
                    ext.sha384,
                    computed_sha384
                )
            }

            temp_file
        } else {
            // For extension images from the old Host Configuration, use the
            // existing file.
            ext.location.clone().with_context(|| {
                format!(
                    "Failed to retrieve current location of extension image '{}'",
                    ext.url
                )
            })?
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

    // Clean-Up: close temporary directory
    temp_mp.close()?;

    Ok(())
}

/// Helper function to extract information from extension-release file
fn read_extension_release(
    mount_point: &Path,
    curr_location: &Path,
    ext: &Extension,
) -> Result<ExtensionData, Error> {
    debug!(
        "Processing extension release file for extension image at '{}'",
        ext.url
    );

    let mut prefix = SYSEXT_PREFIX;
    let sysext_release_dir = mount_point.join(SYSEXT_EXTENSION_RELEASE_DIRECTORY);
    let confext_release_dir = mount_point.join(CONFEXT_EXTENSION_RELEASE_DIRECTORY);

    // Get extension release file
    let dir = match fs::read_dir(&sysext_release_dir) {
        Ok(dir) => dir,
        Err(_) => match fs::read_dir(&confext_release_dir) {
            Ok(dir) => {
                prefix = CONFEXT_PREFIX;
                dir
            }
            Err(_) => return Err(Error::msg("Failed to find extension release file.")),
        },
    }
    .map(|res| res.map(|e| e.path()))
    .collect::<Result<Vec<_>, io::Error>>()?;

    ensure!(
        dir.len() == 1,
        "Expected extension image to have exactly 1 extension-release file, found '{}'",
        dir.len()
    );

    // Read the extension release file
    let path = &dir[0];
    let extension_release_file_content = fs::read_to_string(path).context(format!(
        "Failed to read extension-release file content from file at '{}'",
        &path.display()
    ))?;
    trace!("Found extension release file content:\n{extension_release_file_content}");
    let extension_release_obj = OsRelease::from_str(&extension_release_file_content)
        .with_context(|| "Failed to convert extension release file content to OsRelease object")?;

    // Retrieve SYSEXT_ID or CONFEXT_ID field
    let extension_id = extension_release_obj
        .get_value(&format!("{prefix}ID"))
        .map(|s| s.to_string())
        .ok_or_else(|| Error::msg(format!("Could not find {prefix}ID in extension release")))?;
    let file_name = path
        .display()
        .to_string()
        .split("extension-release.")
        .last()
        .ok_or_else(|| {
            Error::msg("Failed to get extension name from extension release file extension")
        })?
        .to_string();
    let location = match &ext.location {
        Some(location) => location.clone(),
        None => {
            if prefix == SYSEXT_PREFIX {
                PathBuf::from("/var/lib/extensions").join(format!("{file_name}.raw"))
            } else {
                PathBuf::from("/var/lib/confexts").join(format!("{file_name}.raw"))
            }
        }
    };

    Ok(ExtensionData {
        id: extension_id,
        name: file_name,
        sha384: ext.sha384.clone(),
        location,
        temp_location: Some(curr_location.to_path_buf()),
        ext_type: if prefix == SYSEXT_PREFIX {
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
