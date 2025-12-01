use std::{
    collections::{HashMap, HashSet},
    fmt::Display,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{bail, ensure, Context, Error};
use log::{debug, trace, warn};
use tempfile::NamedTempFile;

use osutils::{
    dependencies::{Dependency, DependencyResultExt},
    path,
};
use trident_api::{
    constants::internal_params::HTTP_CONNECTION_TIMEOUT_SECONDS,
    error::{InternalError, ReportError, ServicingError, TridentError},
    primitives::hash::Sha384Hash,
    status::ServicingType,
};

use crate::{
    engine::{EngineContext, Subsystem, RUNS_ON_ALL},
    io_utils::{
        file_reader::FileReader, hashing_reader::HashingReader384, image_streamer::stream_and_hash,
    },
};

mod release;
mod utils;

/// Extension-release
const EXTENSION_RELEASE: &str = "extension-release";

/// Expected extension-release directory for sysexts
const SYSEXT_EXTENSION_RELEASE_DIRECTORY: &str = "/usr/lib/extension-release.d/";
/// Expected extension-release directory for confexts
const CONFEXT_EXTENSION_RELEASE_DIRECTORY: &str = "/etc/extension-release.d/";

/// Temporary directory on target OS for downloading extension images, relative to the newroot mountpoint
const EXTENSION_IMAGE_STAGING_DIRECTORY: &str = "/var/lib/extensions/.staging";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtensionData {
    /// ID of the extension image, corresponding to SYSEXT_ID or CONFEXT_ID in
    /// the extension-release file.
    pub id: String,

    /// Name of the extension image. The file extension of the extension-release
    /// file, i.e. `extension-release.<NAME>`.
    pub name: String,

    /// Hash of the entire extension image.
    pub sha384: Sha384Hash,

    /// Path of the extension image, relative to the target OS.
    pub path: PathBuf,

    /// Path of the extension image, relative to the servicing OS.
    ///
    /// The extension image is downloaded into a temporary location first to
    /// avoid partial or corrupted extensions being merged into the OS.
    pub temp_path: PathBuf,

    /// Sysext or confext.
    pub ext_type: ExtensionType,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
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

#[derive(Default, Debug)]
pub struct ExtensionsSubsystem {
    /// Extension images that should be merged on the target OS.
    extensions: Vec<ExtensionData>,

    /// Extension images that are currently merged on the servicing OS.
    extensions_old: Vec<ExtensionData>,

    /// Directory in which extension images are downloaded.
    staging_dir: PathBuf,
}
impl Subsystem for ExtensionsSubsystem {
    fn name(&self) -> &'static str {
        "extensions"
    }

    fn runs_on(&self, _ctx: &EngineContext) -> &[ServicingType] {
        RUNS_ON_ALL
    }

    // prepare() is only called during runtime updates, so as to download the
    // extension files during Stage.
    fn prepare(&mut self, ctx: &EngineContext) -> Result<(), TridentError> {
        if ctx.servicing_type != ServicingType::RuntimeUpdate {
            debug!("Skipping step 'prepare' because servicing type is not RuntimeUpdate.");
            return Ok(());
        }
        // Define staging directory and ensure that it is empty.
        self.staging_dir = PathBuf::from(EXTENSION_IMAGE_STAGING_DIRECTORY);
        trace!(
            "Defining staging directory for extension images at '{}'",
            self.staging_dir.display()
        );
        self.reset_staging_dir()
            .structured(InternalError::Internal("Failed to reset staging directory"))?;
        // Download new extension images. Mount and process all extension images.
        self.populate_extensions(ctx)
            .structured(InternalError::PopulateExtensionImages)?;
        Ok(())
    }

    // provision() is not called during runtime updates.
    fn provision(&mut self, ctx: &EngineContext, mount_path: &Path) -> Result<(), TridentError> {
        if ctx.servicing_type == ServicingType::RuntimeUpdate {
            debug!("Skipping step 'provision' because servicing type is RuntimeUpdate.");
            return Ok(());
        }
        // Define staging directory and ensure that it is empty.
        self.staging_dir = path::join_relative(mount_path, EXTENSION_IMAGE_STAGING_DIRECTORY);
        trace!(
            "Defining staging directory for extension images at '{}'",
            self.staging_dir.display()
        );
        self.reset_staging_dir()
            .structured(InternalError::Internal("Failed to reset staging directory"))?;
        // Download new extension images. Mount and process all extension images.
        self.populate_extensions(ctx)
            .structured(InternalError::PopulateExtensionImages)?;

        // Ensure that desired target directories exist on the target OS.
        self.create_directories(mount_path)
            .structured(ServicingError::CreateExtensionImageDirectories)?;

        // Determine which images need to be removed and which should be added.
        // Copy extension images to their proper locations.
        self.set_up_extensions(mount_path, ctx.servicing_type)
            .structured(InternalError::SetUpExtensionImages)?;

        Ok(())
    }

    // configure() is only called during runtime updates.
    fn configure(&mut self, ctx: &EngineContext) -> Result<(), TridentError> {
        if ctx.servicing_type != ServicingType::RuntimeUpdate {
            debug!("Skipping step 'configure' because servicing type is not RuntimeUpdate.");
            return Ok(());
        }
        // Ensure that desired target directories exist on the target OS.
        self.create_directories(Path::new("/"))
            .structured(ServicingError::CreateExtensionImageDirectories)?;

        // Determine which images need to be removed and which should be added.
        // Copy extension images to their proper locations.
        self.set_up_extensions(Path::new("/"), ctx.servicing_type)
            .structured(InternalError::SetUpExtensionImages)?;

        // Activate sysexts and confexts on the OS.
        if ctx.spec.os.sysexts != ctx.spec_old.os.sysexts {
            Dependency::SystemdSysext
                .cmd()
                .arg("refresh")
                .run_and_check()
                .message("Failed to refresh sysexts on the OS")?;
        }
        if ctx.spec.os.confexts != ctx.spec_old.os.confexts {
            Dependency::SystemdConfext
                .cmd()
                .arg("refresh")
                .run_and_check()
                .message("Failed to refresh confexts on the OS")?;
        }

        Ok(())
    }

    fn update_host_configuration(&self, ctx: &mut EngineContext) -> Result<(), TridentError> {
        // Update paths of sysexts in the Host Configuration.
        self.extensions
            .iter()
            .filter(|ext| ext.ext_type == ExtensionType::Sysext)
            .try_for_each(|sysext| {
                // Find corresponding sysext in Host Configuration.
                ctx.spec
                    .os
                    .sysexts
                    .iter_mut()
                    .find(|ext| ext.sha384 == sysext.sha384)
                    .structured(InternalError::Internal(
                        "Failed to find previously processed sysext in Host Configuration",
                    ))?
                    .path = Some(sysext.path.clone());
                Ok::<(), TridentError>(())
            })?;

        // Update paths of confexts in the Host Configuration.
        self.extensions
            .iter()
            .filter(|ext| ext.ext_type == ExtensionType::Confext)
            .try_for_each(|confext| {
                // Find corresponding confext in Host Configuration.
                ctx.spec
                    .os
                    .confexts
                    .iter_mut()
                    .find(|ext| ext.sha384 == confext.sha384)
                    .structured(InternalError::Internal(
                        "Failed to find previously processed confext in Host Configuration",
                    ))?
                    .path = Some(confext.path.clone());
                Ok::<(), TridentError>(())
            })?;

        Ok(())
    }

    fn clean_up(&self) -> Result<(), TridentError> {
        // Clean-up staging directory. Recursively remove all contents of
        // staging directory as well as the directory itself.
        self.reset_staging_dir()
            .structured(InternalError::Internal(
                "Failed to remove staging directory",
            ))?;
        Ok(())
    }
}

impl ExtensionsSubsystem {
    fn populate_extensions(&mut self, ctx: &EngineContext) -> Result<(), Error> {
        let timeout = Duration::from_secs(
            ctx.spec
                .internal_params
                .get_u64(HTTP_CONNECTION_TIMEOUT_SECONDS)
                .and_then(|timeout| timeout.ok())
                .unwrap_or(10),
        );

        // Create temporary directory in which to download extension images
        // before copying them to their final path.
        if !self.staging_dir.exists() {
            fs::create_dir_all(&self.staging_dir).with_context(|| {
                format!("Failed to create dir '{}'", self.staging_dir.display())
            })?;
        };

        self.populate_extensions_inner(ctx, timeout, ExtensionType::Sysext, true)?;
        self.populate_extensions_inner(ctx, timeout, ExtensionType::Sysext, false)?;
        self.populate_extensions_inner(ctx, timeout, ExtensionType::Confext, true)?;
        self.populate_extensions_inner(ctx, timeout, ExtensionType::Confext, false)?;
        Ok(())
    }

    /// Updates `self.extensions` or `self.extensions_old`. Takes in 4
    /// arguments:
    /// - self: ExtensionsSubsystem.
    /// - ctx: EngineContext.
    /// - timeout: Time out on HTTP requests.
    /// - ext_type: ExtensionType, indicating which API should be processed.
    /// - new: Boolean indicating whether this function should populate
    ///   `self.extensions` or `self.extensions_old`. When populating
    ///   `self.extensions_old`, expect all extensions in the old Host Configuration
    ///   to be present on the servicing OS so we will not download any new images.
    fn populate_extensions_inner(
        &mut self,
        ctx: &EngineContext,
        timeout: Duration,
        ext_type: ExtensionType,
        new: bool,
    ) -> Result<(), Error> {
        let hc_extensions = match (new, &ext_type) {
            (true, ExtensionType::Sysext) => &ctx.spec.os.sysexts,
            (false, ExtensionType::Sysext) => &ctx.spec_old.os.sysexts,
            (true, ExtensionType::Confext) => &ctx.spec.os.confexts,
            (false, ExtensionType::Confext) => &ctx.spec_old.os.confexts,
        };

        for ext in hc_extensions {
            let extension_file = if new {
                // First, check if this extension already exists on the system.
                if let Some(existing_file_path) = match &ext_type {
                    ExtensionType::Sysext => {
                        utils::check_for_existing_image(ext, &ctx.spec_old.os.sysexts)
                    }
                    ExtensionType::Confext => {
                        utils::check_for_existing_image(ext, &ctx.spec_old.os.confexts)
                    }
                } {
                    // Check if Trident is running in a container, and adjust path accordingly.
                    let adjusted_path =
                        utils::adjust_path_if_container(existing_file_path.clone())?;
                    // Ensure that file exists.
                    ensure!(
                        adjusted_path.exists(),
                        "Expected to find extension image from URL '{}' at path '{}' based on previous Host Configuration, but path does not exist",
                        ext.url,
                        existing_file_path.display() // Display the unadjusted path for readability
                    );
                    adjusted_path
                } else {
                    // The extension is new to the OS, so we need to download it.
                    // Create and persist a temporary file; get its path.
                    let temp_file: PathBuf = NamedTempFile::new_in(&self.staging_dir)
                        .context("Failed to create temporary file")?
                        .into_temp_path()
                        .keep()
                        .context("Failed to persist temporary file")?;

                    // Download the extension image to this temporary file.
                    let reader = FileReader::new(&ext.url, timeout)
                        .context("Failed to create file reader")?
                        .complete_reader()
                        .context("Failed to create complete file reader")?;
                    let hash_reader = HashingReader384::new(reader);
                    let computed_sha384 = stream_and_hash(hash_reader, &temp_file)
                        .context("Failed to download extension image and calculate its hash")?;

                    // Ensure computed SHA384 matches SHA384 in Host Configuration.
                    if ext.sha384 != computed_sha384 {
                        bail!(
                            "SHA384 mismatch for extension image at '{}': expected {}, got {}",
                            ext.url,
                            ext.sha384,
                            computed_sha384
                        )
                    }

                    temp_file
                }
            } else {
                // For extension images from the old Host Configuration, use the
                // existing file.
                let path = ext.path.clone().with_context(|| {
                    format!(
                        "Failed to retrieve current path of extension image '{}'",
                        ext.url
                    )
                })?;
                // Check if Trident is running in a container, and adjust path accordingly.
                let adjusted_path = utils::adjust_path_if_container(path.clone())?;
                // Ensure that file exists
                ensure!(
                    adjusted_path.exists(),
                    "Expected to find extension image from URL '{}' at path '{}', but path does not exist",
                    ext.url,
                    path.display() // Display unadjusted path for readability
                );
                adjusted_path
            };

            // Create temporary mountpoint, which will be used to read the extension-release file
            let temp_mp = tempfile::tempdir()?;

            // Attach a device and mount the extension
            let device_path = utils::attach_device_and_mount(&extension_file, temp_mp.path())
                .context("Failed to mount")?;

            // Get extension-release file
            let ext_data_result =
                release::read_extension_release(temp_mp.path(), &extension_file, ext, &ext_type);

            // Clean-Up: unmount and detach the device
            utils::detach_device_and_unmount(device_path, temp_mp.path())
                .context("Failed to unmount")?;

            let ext_data =
                ext_data_result.context("Failed to get extension-release information")?;
            if new {
                self.extensions.push(ext_data);
            } else {
                self.extensions_old.push(ext_data);
            }
        }

        Ok(())
    }

    /// Ensures that all target directories for extension images exist on the
    /// target OS.
    fn create_directories(&self, mount_path: &Path) -> Result<(), Error> {
        let mut seen_dirs = HashSet::new();

        self.extensions.iter().try_for_each(|ext| {
            let dir = ext.path.parent().with_context(|| {
                format!(
                    "Failed to get parent directory of path '{}'",
                    ext.path.display()
                )
            })?;
            if seen_dirs.insert(dir) {
                fs::create_dir_all(path::join_relative(mount_path, dir)).with_context(|| {
                    format!(
                        "Failed to create directory '{}' on the target OS at mount path '{}'",
                        dir.display(),
                        mount_path.display()
                    )
                })?;
            }
            Ok(())
        })
    }

    /// Identifies which extension images should be added to the target OS from
    /// the set of extensions on the servicing OS and the set of newly
    /// downloaded extensions.
    /// - New extensions that exist *only* in the new HC are renamed/copied from
    ///   the staging directory to their target paths on the target OS.
    /// - Extensions that exist *only* in the old HC are removed from the
    ///   servicing OS (only during a runtime update, i.e. target OS ==
    ///   servicing OS).
    /// - Extensions that exist in both the new HC and the old HC:
    ///   - If the hash differs, then the new version of the extension is
    ///     renamed/copied from the staging directory to its path on the target
    ///     OS. The old version of the extension is removed from the servicing
    ///     OS if servicing type is not Clean Install or A/B update.
    ///   - If the hash is the same, the extension is renamed/copied from its
    ///     old location on the servicing OS to its new location on the target
    ///     OS.
    fn set_up_extensions(
        &self,
        mount_path: &Path,
        servicing_type: ServicingType,
    ) -> Result<(), Error> {
        let old_exts_hashmap: HashMap<_, _> = self
            .extensions_old
            .iter()
            .map(|ext| ((ext.id.clone(), ext.ext_type.clone()), ext))
            .collect();
        let old_exts_ids: HashSet<_> = old_exts_hashmap.keys().cloned().collect();

        let new_exts_hashmap: HashMap<_, _> = self
            .extensions
            .iter()
            .map(|ext| ((ext.id.clone(), ext.ext_type.clone()), ext))
            .collect();
        let new_exts_ids: HashSet<_> = new_exts_hashmap.keys().cloned().collect();

        let mut ids_to_add: Vec<_> = new_exts_ids.difference(&old_exts_ids).cloned().collect();
        let mut ids_to_remove: Vec<_> = old_exts_ids.difference(&new_exts_ids).cloned().collect();

        // Identify extension images that should be updated.
        for id in new_exts_ids.intersection(&old_exts_ids) {
            // Check hash
            let old_hash = &old_exts_hashmap[id].sha384;
            let new_hash = &new_exts_hashmap[id].sha384;

            ids_to_add.push(id.clone());
            if old_hash != new_hash {
                ids_to_remove.push(id.clone());
            }
        }

        let extensions_to_add: Vec<_> = new_exts_hashmap
            .iter()
            .filter(|(k, _)| ids_to_add.contains(k))
            .map(|(_, ext)| ext)
            .collect();
        let extensions_to_remove: Vec<_> = old_exts_hashmap
            .iter()
            .filter(|(k, _)| ids_to_remove.contains(k))
            .map(|(_, ext)| ext)
            .collect();

        // Add new extensions that should be added
        for ext in extensions_to_add {
            let new_path = path::join_relative(mount_path, &ext.path);
            trace!(
                "Copying {} '{}' to path {}",
                ext.ext_type,
                ext.name,
                new_path.display()
            );
            // Attempt atomic rename first, for extensions that were newly
            // downloaded to the staging directory.
            if let Err(e) = fs::rename(&ext.temp_path, &new_path) {
                warn!(
                    "Failed to atomically rename '{}' to '{}': {e}. Attempting file copy instead.",
                    ext.temp_path.display(),
                    new_path.display()
                );
                // Fall back to file copy if this fails, i.e. if the files are
                // not on the same filesystem. This will be the default for
                // extensions existing on the servicing OS.
                fs::copy(&ext.temp_path, &new_path).context(format!(
                    "Failed to copy extension image from '{}' to '{}'",
                    ext.temp_path.display(),
                    new_path.display()
                ))?;
            }
        }

        // On Clean Install and A/B Update, it is not necessary to remove
        // extensions from the servicing OS as these will not be present on the
        // target OS. (We also do not expect any existing extension images on
        // the servicing OS for Clean Install.)
        if !(servicing_type == ServicingType::CleanInstall
            || servicing_type == ServicingType::AbUpdate)
        {
            // Otherwise, remove existing extensions that are not in the new
            // Host Configuration.
            for ext in extensions_to_remove {
                // Check that file still exists. If the file was renamed in the
                // step above, there is no need to remove it. Note that for any
                // extension in 'extensions_to_remove', ext.path and
                // ext.temp_path will be the same because each ExtensionData
                // object in the vector was populated from the old Host
                // Configuration.
                if ext.temp_path.exists() {
                    fs::remove_file(&ext.temp_path).with_context(|| {
                        format!("Failed to delete file at '{}'", ext.temp_path.display())
                    })?;
                }
            }
        }

        Ok(())
    }

    fn reset_staging_dir(&self) -> Result<(), Error> {
        if self.staging_dir.is_dir() {
            fs::remove_dir_all(&self.staging_dir)?;
        };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::TempDir;
    use url::Url;

    use ::trident_api::config::Extension;

    #[test]
    fn test_populate_extensions_empty() {
        // Test with no extensions
        let mut subsystem = ExtensionsSubsystem {
            staging_dir: TempDir::new().unwrap().path().to_path_buf(),
            ..Default::default()
        };
        let ctx = EngineContext::default();
        subsystem.populate_extensions(&ctx).unwrap();

        assert!(
            subsystem.extensions.is_empty(),
            "ExtensionsSubsystem extensions should be empty when there are no extensions in the Host Configuration"
        );
        assert!(
            subsystem.extensions_old.is_empty(),
            "ExtensionsSubsystem extensions_old should be empty when there are no extensions in the old Host Configuration"
        );
    }

    #[test]
    fn test_create_directories() {
        let subsystem = ExtensionsSubsystem {
            extensions: vec![
                // Sysext in /etc/extensions
                ExtensionData {
                    id: "sysext1".to_string(),
                    name: "sysext1".to_string(),
                    sha384: Sha384Hash::from("a".repeat(96)),
                    path: PathBuf::from("/etc/extensions/sysext1.raw"),
                    temp_path: PathBuf::from("/var/lib/extensions/.staging/sysext1.raw"),
                    ext_type: ExtensionType::Sysext,
                },
                // Sysext in /var/lib/extensions (default)
                ExtensionData {
                    id: "sysext2".to_string(),
                    name: "sysext2".to_string(),
                    sha384: Sha384Hash::from("b".repeat(96)),
                    path: PathBuf::from("/var/lib/extensions/sysext2.raw"),
                    temp_path: PathBuf::from("/var/lib/extensions/.staging/sysext2.raw"),
                    ext_type: ExtensionType::Sysext,
                },
                // Sysext in /.extra/sysext
                ExtensionData {
                    id: "sysext3".to_string(),
                    name: "sysext3".to_string(),
                    sha384: Sha384Hash::from("c".repeat(96)),
                    path: PathBuf::from("/.extra/sysext/sysext3.raw"),
                    temp_path: PathBuf::from("/var/lib/extensions/.staging/sysext3.raw"),
                    ext_type: ExtensionType::Sysext,
                },
                // Confext in /var/lib/confexts (default)
                ExtensionData {
                    id: "confext1".to_string(),
                    name: "confext1".to_string(),
                    sha384: Sha384Hash::from("d".repeat(96)),
                    path: PathBuf::from("/var/lib/confexts/confext1.raw"),
                    temp_path: PathBuf::from("/var/lib/extensions/.staging/confext1.raw"),
                    ext_type: ExtensionType::Confext,
                },
                // Confext in /usr/lib/confexts
                ExtensionData {
                    id: "confext2".to_string(),
                    name: "confext2".to_string(),
                    sha384: Sha384Hash::from("e".repeat(96)),
                    path: PathBuf::from("/usr/lib/confexts/confext2.raw"),
                    temp_path: PathBuf::from("/var/lib/extensions/.staging/confext2.raw"),
                    ext_type: ExtensionType::Confext,
                },
                // Confext in /usr/local/lib/confexts
                ExtensionData {
                    id: "confext3".to_string(),
                    name: "confext3".to_string(),
                    sha384: Sha384Hash::from("f".repeat(96)),
                    path: PathBuf::from("/usr/local/lib/confexts/confext3.raw"),
                    temp_path: PathBuf::from("/var/lib/extensions/.staging/confext3.raw"),
                    ext_type: ExtensionType::Confext,
                },
            ],
            ..Default::default()
        };

        let mount_path = TempDir::new().unwrap();
        assert!(!mount_path.path().join("etc/extensions").exists());
        assert!(!mount_path.path().join("var/lib/extensions").exists());
        assert!(!mount_path.path().join(".extra/sysext").exists());
        assert!(!mount_path.path().join("var/lib/confexts").exists());
        assert!(!mount_path.path().join("usr/lib/confexts").exists());
        assert!(!mount_path.path().join("usr/local/lib/confexts").exists());

        subsystem.create_directories(mount_path.path()).unwrap();
        assert!(mount_path.path().join("etc/extensions").exists());
        assert!(mount_path.path().join("var/lib/extensions").exists());
        assert!(mount_path.path().join(".extra/sysext").exists());
        assert!(mount_path.path().join("var/lib/confexts").exists());
        assert!(mount_path.path().join("usr/lib/confexts").exists());
        assert!(mount_path.path().join("usr/local/lib/confexts").exists());
    }

    #[test]
    fn test_update_host_configuration_sysexts() {
        let mut ctx = EngineContext::default();
        ctx.spec.os.sysexts = vec![
            Extension {
                url: Url::parse("https://example.com/sysext1.raw").unwrap(),
                sha384: Sha384Hash::from("a".repeat(96)),
                path: None,
            },
            Extension {
                url: Url::parse("https://example.com/sysext2.raw").unwrap(),
                sha384: Sha384Hash::from("b".repeat(96)),
                path: Some(PathBuf::from("/etc/extensions/sysext2.raw")),
            },
        ];

        let subsystem = ExtensionsSubsystem {
            extensions: vec![
                ExtensionData {
                    id: "sysext1".to_string(),
                    name: "sysext1".to_string(),
                    sha384: Sha384Hash::from("a".repeat(96)),
                    path: PathBuf::from("/var/lib/extensions/sysext1.raw"),
                    temp_path: PathBuf::from(EXTENSION_IMAGE_STAGING_DIRECTORY).join("sysext1.raw"),

                    ext_type: ExtensionType::Sysext,
                },
                ExtensionData {
                    id: "sysext2".to_string(),
                    name: "sysext2".to_string(),
                    sha384: Sha384Hash::from("b".repeat(96)),
                    path: PathBuf::from("/etc/extensions/sysext2.raw"),
                    temp_path: PathBuf::from(EXTENSION_IMAGE_STAGING_DIRECTORY).join("sysext2.raw"),

                    ext_type: ExtensionType::Sysext,
                },
            ],
            ..Default::default()
        };
        subsystem.update_host_configuration(&mut ctx).unwrap();

        for i in 0..subsystem.extensions.len() {
            assert_eq!(
                ctx.spec.os.sysexts[i].path,
                Some(subsystem.extensions[i].path.clone())
            )
        }
    }

    #[test]
    fn test_update_host_configuration_confexts() {
        let mut ctx = EngineContext::default();
        ctx.spec.os.confexts = vec![
            Extension {
                url: Url::parse("https://example.com/confext1.raw").unwrap(),
                sha384: Sha384Hash::from("a".repeat(96)),
                path: None,
            },
            Extension {
                url: Url::parse("https://example.com/confext2.raw").unwrap(),
                sha384: Sha384Hash::from("b".repeat(96)),
                path: Some(PathBuf::from("/usr/lib/confexts/confext2.raw")),
            },
        ];

        let subsystem = ExtensionsSubsystem {
            extensions: vec![
                ExtensionData {
                    id: "confext1".to_string(),
                    name: "confext1".to_string(),
                    sha384: Sha384Hash::from("a".repeat(96)),
                    path: PathBuf::from("/var/lib/confexts/confext1.raw"),
                    temp_path: PathBuf::from(EXTENSION_IMAGE_STAGING_DIRECTORY)
                        .join("confext1.raw"),
                    ext_type: ExtensionType::Confext,
                },
                ExtensionData {
                    id: "confext2".to_string(),
                    name: "confext2".to_string(),
                    sha384: Sha384Hash::from("b".repeat(96)),
                    path: PathBuf::from("/usr/lib/confexts/confext2.raw"),
                    temp_path: PathBuf::from("/var/lib/extensions/.staging/confext2.raw"),
                    ext_type: ExtensionType::Confext,
                },
            ],
            ..Default::default()
        };
        subsystem.update_host_configuration(&mut ctx).unwrap();

        for i in 0..subsystem.extensions.len() {
            assert_eq!(
                ctx.spec.os.confexts[i].path,
                Some(subsystem.extensions[i].path.clone())
            )
        }
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;

    use sha2::{Digest, Sha384};
    use tempfile::{env::temp_dir, TempDir};
    use url::Url;

    use osutils::{
        filesystems::{MkfsFileSystemType, MountFileSystemType},
        mkfs, mount,
    };
    use pytest_gen::functional_test;
    use trident_api::{
        config::Extension,
        constants::{DEFAULT_CONFEXT_DIRECTORY, DEFAULT_SYSEXT_DIRECTORY},
    };

    /// Helper to create a minimal Discoverable Disk Image extension for testing
    fn create_test_extension_image(
        image_path: &Path,
        ext_name: &str,
        ext_type: &ExtensionType,
        ext_release_content: &str,
    ) -> Sha384Hash {
        // Format it as ext4
        Dependency::Mkfs
            .cmd()
            .args([
                "-t",
                "ext4",
                "-q",
                "-L",
                ext_name,
                image_path.to_str().unwrap(),
                "1M",
            ])
            .run_and_check()
            .unwrap();

        // Mount temporarily to write extension-release file
        let mount_point = TempDir::new().unwrap();
        Dependency::Mount
            .cmd()
            .args([
                "-o",
                "loop",
                image_path.to_str().unwrap(),
                mount_point.path().to_str().unwrap(),
            ])
            .run_and_check()
            .unwrap();

        // Copy the extension-release file structure
        let release_subdir = match ext_type {
            ExtensionType::Sysext => SYSEXT_EXTENSION_RELEASE_DIRECTORY,
            ExtensionType::Confext => CONFEXT_EXTENSION_RELEASE_DIRECTORY,
        };

        // Create a temporary directory for the extension content
        let release_dir = path::join_relative(mount_point.path(), release_subdir);
        fs::create_dir_all(&release_dir).unwrap();

        let release_file_path = release_dir.join(format!("{EXTENSION_RELEASE}.{ext_name}"));
        fs::write(&release_file_path, ext_release_content).unwrap();

        // Unmount
        Dependency::Umount
            .cmd()
            .arg(mount_point.path().to_str().unwrap())
            .run_and_check()
            .unwrap();

        // Compute SHA384 hash
        let image_contents = fs::read(image_path).unwrap();
        Sha384Hash::from(format!("{:x}", Sha384::digest(&image_contents)))
    }

    fn create_test_extensions(
        input: &[(Option<PathBuf>, &str, ExtensionType, &str, bool)],
    ) -> EngineContext {
        let mut output = EngineContext::default();
        for (file_path, ext_name, ext_type, ext_release_content, new) in input {
            let path = match file_path {
                Some(path) => path.clone(),
                None => NamedTempFile::new()
                    .unwrap()
                    .into_temp_path()
                    .keep()
                    .unwrap(),
            };
            let test_ext_hash =
                create_test_extension_image(&path, ext_name, ext_type, ext_release_content);
            match (ext_type, new) {
                (ExtensionType::Sysext, true) => output.spec.os.sysexts.push(Extension {
                    url: Url::from_file_path(path).unwrap(),
                    sha384: test_ext_hash,
                    path: file_path.clone(),
                }),
                (ExtensionType::Confext, true) => output.spec.os.confexts.push(Extension {
                    url: Url::from_file_path(path).unwrap(),
                    sha384: test_ext_hash,
                    path: file_path.clone(),
                }),
                (ExtensionType::Sysext, false) => output.spec_old.os.sysexts.push(Extension {
                    url: Url::from_file_path(path).unwrap(),
                    sha384: test_ext_hash,
                    path: file_path.clone(),
                }),
                (ExtensionType::Confext, false) => output.spec_old.os.confexts.push(Extension {
                    url: Url::from_file_path(path).unwrap(),
                    sha384: test_ext_hash,
                    path: file_path.clone(),
                }),
            }
        }
        output
    }

    #[functional_test]
    fn test_populate_extensions_new_success() {
        // Create test extension images
        let test_inputs = [
            (
                None,
                "my_sysext",
                ExtensionType::Sysext,
                "ID=_any\nSYSEXT_ID=my_sysext",
                true,
            ),
            (
                None,
                "my_confext",
                ExtensionType::Confext,
                "ID=_any\nCONFEXT_ID=my_confext",
                true,
            ),
        ];

        // Process extensions
        let ctx = create_test_extensions(&test_inputs);
        let mut subsystem = ExtensionsSubsystem {
            staging_dir: temp_dir(),
            ..Default::default()
        };
        subsystem.populate_extensions(&ctx).unwrap();

        // Verify results
        let subsystem_extensions = subsystem.extensions;
        let mut hc_extensions = ctx.spec.os.sysexts.clone();
        hc_extensions.extend(ctx.spec.os.confexts.clone());
        assert_eq!(hc_extensions.len(), subsystem_extensions.len());
        for (((_, name, expected_type, _, _), hc_ext), subsystem_ext) in test_inputs
            .iter()
            .zip(&hc_extensions)
            .zip(&subsystem_extensions)
        {
            assert_eq!(subsystem_ext.ext_type, *expected_type);
            assert_eq!(subsystem_ext.id, *name);
            assert_eq!(subsystem_ext.name, *name);
            assert_eq!(subsystem_ext.sha384, hc_ext.sha384);

            // Verify default path was set correctly
            let expected_dir = match expected_type {
                ExtensionType::Sysext => DEFAULT_SYSEXT_DIRECTORY,
                ExtensionType::Confext => DEFAULT_CONFEXT_DIRECTORY,
            };
            assert_eq!(
                subsystem_ext.path,
                PathBuf::from(expected_dir).join(format!("{}.raw", subsystem_ext.name))
            );
        }
    }

    #[functional_test]
    fn test_populate_extensions_existing_success() {
        // Create temporary test locations; note that 'populate' function does
        // not check the validity of the location as this happens in static and
        // dynamic validation.
        let temp_file1 = NamedTempFile::new()
            .unwrap()
            .into_temp_path()
            .keep()
            .unwrap();
        let temp_file2 = NamedTempFile::new()
            .unwrap()
            .into_temp_path()
            .keep()
            .unwrap();

        // Create test extension images
        let test_inputs = [
            (
                Some(temp_file1),
                "my_sysext",
                ExtensionType::Sysext,
                "ID=_any\nSYSEXT_ID=my_sysext",
                false,
            ),
            (
                Some(temp_file2),
                "my_confext",
                ExtensionType::Confext,
                "ID=_any\nCONFEXT_ID=my_confext",
                false,
            ),
        ];

        // Process extensions with new=false
        let ctx = create_test_extensions(&test_inputs);
        let mut subsystem = ExtensionsSubsystem {
            staging_dir: temp_dir(),
            ..Default::default()
        };
        subsystem.populate_extensions(&ctx).unwrap();

        // Verify results
        let subsystem_extensions = subsystem.extensions_old;
        let mut hc_extensions = ctx.spec_old.os.sysexts.clone();
        hc_extensions.extend(ctx.spec_old.os.confexts.clone());
        assert_eq!(hc_extensions.len(), subsystem_extensions.len());
        for (((_, name, expected_type, _, _), hc_ext), subsystem_ext) in test_inputs
            .iter()
            .zip(&hc_extensions)
            .zip(&subsystem_extensions)
        {
            assert_eq!(subsystem_ext.ext_type, *expected_type);
            assert_eq!(subsystem_ext.id, *name);
            assert_eq!(subsystem_ext.name, *name);
            assert_eq!(&subsystem_ext.sha384, &hc_ext.sha384);
            assert_eq!(&subsystem_ext.path, hc_ext.path.as_ref().unwrap());
        }
    }

    #[functional_test]
    fn test_populate_extensions_sha384_mismatch() {
        // Create an extension image
        let temp_file = NamedTempFile::new()
            .unwrap()
            .into_temp_path()
            .keep()
            .unwrap();
        let actual_hash = create_test_extension_image(
            &temp_file,
            "test_ext",
            &ExtensionType::Sysext,
            "ID=_any\nSYSEXT_ID=test_ext",
        );

        // Create Extension with incorrect hash
        let wrong_hash = Sha384Hash::from("a".repeat(96));
        let extension_url = Url::from_file_path(&temp_file).unwrap();
        let hc_extension = Extension {
            url: extension_url.clone(),
            sha384: wrong_hash.clone(),
            path: None,
        };

        // Attempt to process - should fail due to hash mismatch
        let mut ctx = EngineContext::default();
        ctx.spec.os.sysexts = vec![hc_extension];
        let mut subsystem = ExtensionsSubsystem {
            staging_dir: temp_dir(),
            ..Default::default()
        };
        let error = subsystem.populate_extensions(&ctx).unwrap_err().to_string();

        assert_eq!(error, format!("SHA384 mismatch for extension image at '{extension_url}': expected {wrong_hash}, got {actual_hash}"));
    }

    // Location of existing ext doesn't exist
    #[functional_test]
    fn test_populate_extensions_nonexistent_path() {
        let temp_file = NamedTempFile::new()
            .unwrap()
            .into_temp_path()
            .keep()
            .unwrap();
        let hash = create_test_extension_image(
            &temp_file,
            "test_ext",
            &ExtensionType::Sysext,
            "ID=_any\nSYSEXT_ID=test_ext",
        );

        // Create Extension
        let ext_url = Url::from_file_path(&temp_file).unwrap();
        let ext_path = PathBuf::from("/etc/extensions/test_ext.raw"); // No file exists at this path
        let hc_extension = Extension {
            url: ext_url.clone(),
            sha384: hash,
            path: Some(ext_path.clone()),
        };

        // Attempt to process as an existing Extension
        let mut ctx = EngineContext::default();
        ctx.spec_old.os.sysexts = vec![hc_extension];
        let mut subsystem = ExtensionsSubsystem {
            staging_dir: temp_dir(),
            ..Default::default()
        };
        let error = subsystem.populate_extensions(&ctx).unwrap_err().to_string();

        assert_eq!(error, format!("Expected to find extension image from URL '{ext_url}' at path '{}', but path does not exist", ext_path.display()));
    }

    #[functional_test]
    fn test_set_up_extensions_add() {
        // Test adding new extensions
        let mount_path = TempDir::new().unwrap();
        let staging_dir = path::join_relative(mount_path.path(), EXTENSION_IMAGE_STAGING_DIRECTORY);
        fs::create_dir_all(&staging_dir).unwrap();

        // Create a new extension in staging directory
        let temp_file = NamedTempFile::new_in(&staging_dir).unwrap();
        let ext_hash = create_test_extension_image(
            temp_file.path(),
            "new_ext",
            &ExtensionType::Sysext,
            "ID=_any\nSYSEXT_ID=new_ext",
        );

        let target_path = PathBuf::from("/var/lib/extensions/new_ext.raw");
        let subsystem = ExtensionsSubsystem {
            extensions: vec![ExtensionData {
                id: "new_ext".to_string(),
                name: "new_ext".to_string(),
                sha384: ext_hash,
                path: target_path.clone(),
                temp_path: temp_file.path().to_path_buf(),
                ext_type: ExtensionType::Sysext,
            }],
            ..Default::default()
        };

        // Create necessary directories
        subsystem.create_directories(mount_path.path()).unwrap();

        // Run set_up_extensions
        subsystem
            .set_up_extensions(mount_path.path(), ServicingType::CleanInstall)
            .unwrap();

        // Verify the extension was copied to the target location
        assert!(
            path::join_relative(mount_path.path(), &target_path).exists(),
            "Extension should be copied to target"
        );
        // Verify that fs::rename was used, so temporary file should not exist anymore.
        assert!(
            !temp_file.path().exists(),
            "Temporary extension file should no longer exist."
        );
    }

    #[functional_test]
    fn test_set_up_extensions_add_duplicate_id() {
        // Test adding new extensions where a sysext and confext have the same ID
        let mount_path = TempDir::new().unwrap();
        let staging_dir = path::join_relative(mount_path.path(), EXTENSION_IMAGE_STAGING_DIRECTORY);
        fs::create_dir_all(&staging_dir).unwrap();

        // Create a sysext and confext with the same "ID"
        let sysext_file = NamedTempFile::new_in(&staging_dir).unwrap();
        let sysext_hash = create_test_extension_image(
            sysext_file.path(),
            "new_ext",
            &ExtensionType::Sysext,
            "ID=_any\nSYSEXT_ID=new_ext",
        );
        let confext_file = NamedTempFile::new_in(&staging_dir).unwrap();
        let confext_hash = create_test_extension_image(
            confext_file.path(),
            "new_ext",
            &ExtensionType::Confext,
            "ID=_any\nCONFEXT_ID=new_ext",
        );

        let sysext_target_path = PathBuf::from("/etc/extensions/new_ext.raw");
        let confext_target_path = PathBuf::from("/usr/lib/confexts/new_ext.raw");
        let subsystem = ExtensionsSubsystem {
            extensions: vec![
                ExtensionData {
                    id: "new_ext".to_string(),
                    name: "new_ext".to_string(),
                    sha384: sysext_hash,
                    path: sysext_target_path.clone(),
                    temp_path: sysext_file.path().to_path_buf(),
                    ext_type: ExtensionType::Sysext,
                },
                ExtensionData {
                    id: "new_ext".to_string(),
                    name: "new_ext".to_string(),
                    sha384: confext_hash,
                    path: confext_target_path.clone(),
                    temp_path: confext_file.path().to_path_buf(),
                    ext_type: ExtensionType::Confext,
                },
            ],
            ..Default::default()
        };

        // Create necessary directories
        subsystem.create_directories(mount_path.path()).unwrap();

        // Run set_up_extensions
        subsystem
            .set_up_extensions(mount_path.path(), ServicingType::CleanInstall)
            .unwrap();

        // Verify the extensions were copied to their target locations
        assert!(
            path::join_relative(mount_path.path(), &sysext_target_path).exists(),
            "Sysext should be copied to target OS"
        );
        assert!(
            path::join_relative(mount_path.path(), &confext_target_path).exists(),
            "Confext should be copied to target OS"
        );

        // Verify that the temporary extension images don't exist anymore (were
        // renamed).
        assert!(
            !sysext_file.path().exists(),
            "Temporary sysext file should not exist anymore"
        );
        assert!(
            !confext_file.path().exists(),
            "Temporary confext file should not exist anymore"
        );
    }

    #[functional_test]
    fn test_set_up_extensions_remove_old() {
        // Create an old extension file
        let old_ext_dir = TempDir::new().unwrap();
        let old_ext = NamedTempFile::new_in(&old_ext_dir).unwrap();
        let ext_hash = create_test_extension_image(
            old_ext.path(),
            "old_ext",
            &ExtensionType::Sysext,
            "ID=_any\nSYSEXT_ID=old_ext",
        );
        let subsystem = ExtensionsSubsystem {
            extensions_old: vec![ExtensionData {
                id: "old_ext".to_string(),
                name: "old_ext".to_string(),
                sha384: ext_hash,
                path: old_ext.path().to_path_buf(),
                temp_path: old_ext.path().to_path_buf(),
                ext_type: ExtensionType::Sysext,
            }],
            ..Default::default()
        };

        let mount_path = TempDir::new().unwrap();
        // Create necessary directories
        subsystem.create_directories(mount_path.path()).unwrap();
        // Run set_up_extensions with A/B update (should NOT remove old extensions)
        subsystem
            .set_up_extensions(mount_path.path(), ServicingType::AbUpdate)
            .unwrap();
        // Verify the extension still exists
        assert!(
            old_ext.path().exists(),
            "Old extension should still exist in its original location"
        );
        // Verify that nothing has been copied to the target OS.
        assert!(mount_path.path().read_dir().unwrap().next().is_none());

        // Run set_up_extensions with RuntimeUpdate (should remove old extensions)
        subsystem
            .set_up_extensions(mount_path.path(), ServicingType::RuntimeUpdate)
            .unwrap();
        // Verify the extension was removed
        assert!(!old_ext.path().exists(), "Old extension should be removed");
        // Verify that nothing has been copied to the target OS.
        assert!(mount_path.path().read_dir().unwrap().next().is_none());
    }

    #[functional_test]
    fn test_set_up_extensions_update_replace_ab_update() {
        // Test scenario where an old sysext and a new sysext match on ID, and
        // an update is required (mismatched hashes), on an A/B update.
        // Create old extension.
        let old_ext_dir = TempDir::new().unwrap();
        let old_ext = NamedTempFile::new_in(old_ext_dir.path()).unwrap();
        let old_hash = create_test_extension_image(
            old_ext.path(),
            "old_ext",
            &ExtensionType::Sysext,
            "ID=_any\nSYSEXT_ID=my_ext",
        );

        // Create new version with different content
        let mount_path = TempDir::new().unwrap();
        let staging_dir = path::join_relative(mount_path.path(), EXTENSION_IMAGE_STAGING_DIRECTORY);
        fs::create_dir_all(&staging_dir).unwrap();
        let new_ext = NamedTempFile::new_in(&staging_dir).unwrap();
        let new_hash = create_test_extension_image(
            new_ext.path(),
            "updated_ext",
            &ExtensionType::Sysext,
            "ID=_any\nSYSEXT_ID=my_ext",
        );

        let target_path = "/var/lib/extensions/updated_ext.raw";
        let subsystem = ExtensionsSubsystem {
            extensions: vec![ExtensionData {
                id: "my_ext".to_string(), // Matching ID
                name: "updated_ext".to_string(),
                sha384: new_hash,
                path: PathBuf::from(target_path),
                temp_path: new_ext.path().to_path_buf(),
                ext_type: ExtensionType::Sysext,
            }],
            extensions_old: vec![ExtensionData {
                id: "my_ext".to_string(), // Matching ID
                name: "old_ext".to_string(),
                sha384: old_hash,
                path: old_ext.path().to_path_buf(),
                temp_path: old_ext.path().to_path_buf(),
                ext_type: ExtensionType::Sysext,
            }],
            ..Default::default()
        };

        // Create necessary directories
        subsystem.create_directories(mount_path.path()).unwrap();
        // Run set_up_extensions; A/B update
        subsystem
            .set_up_extensions(mount_path.path(), ServicingType::AbUpdate)
            .unwrap();

        // Verify old extension was NOT removed, since servicing type is A/B update
        assert!(
            old_ext.path().exists(),
            "Old extension should not be removed from the servicing OS"
        );

        // Verify new extension was copied
        assert!(
            path::join_relative(mount_path.path(), target_path).exists(),
            "New extension should be copied"
        );
    }

    #[functional_test]
    fn test_set_up_extensions_update_replace_runtime_update() {
        // Test scenario where an old sysext and a new sysext match on ID, and
        // an update is required (mismatched hashes), on a runtime update.
        // Create old extension.
        let old_ext_dir = TempDir::new().unwrap();
        let old_ext = NamedTempFile::new_in(old_ext_dir.path()).unwrap();
        let old_hash = create_test_extension_image(
            old_ext.path(),
            "old_ext",
            &ExtensionType::Sysext,
            "ID=_any\nSYSEXT_ID=my_ext",
        );

        // Create new version with different content
        let mount_path = TempDir::new().unwrap();
        let staging_dir = path::join_relative(mount_path.path(), EXTENSION_IMAGE_STAGING_DIRECTORY);
        fs::create_dir_all(&staging_dir).unwrap();
        let new_ext = NamedTempFile::new_in(&staging_dir).unwrap();
        let new_hash = create_test_extension_image(
            new_ext.path(),
            "updated_ext",
            &ExtensionType::Sysext,
            "ID=_any\nSYSEXT_ID=my_ext",
        );

        let target_path = "/var/lib/extensions/updated_ext.raw";
        let subsystem = ExtensionsSubsystem {
            extensions: vec![ExtensionData {
                id: "my_ext".to_string(), // Matching ID
                name: "updated_ext".to_string(),
                sha384: new_hash,
                path: PathBuf::from(target_path),
                temp_path: new_ext.path().to_path_buf(),
                ext_type: ExtensionType::Sysext,
            }],
            extensions_old: vec![ExtensionData {
                id: "my_ext".to_string(), // Matching ID
                name: "old_ext".to_string(),
                sha384: old_hash,
                path: old_ext.path().to_path_buf(),
                temp_path: old_ext.path().to_path_buf(),
                ext_type: ExtensionType::Sysext,
            }],
            ..Default::default()
        };

        // Create necessary directories
        subsystem.create_directories(mount_path.path()).unwrap();
        // Run set_up_extensions; runtime update
        subsystem
            .set_up_extensions(mount_path.path(), ServicingType::RuntimeUpdate)
            .unwrap();

        // Verify old extension was removed, since servicing type is not A/B
        // update (or Clean Install).
        assert!(
            !old_ext.path().exists(),
            "Old extension should be removed from the servicing OS"
        );

        // Verify new extension exists on the target OS
        assert!(
            path::join_relative(mount_path.path(), target_path).exists(),
            "New extension should be copied"
        );
    }

    #[functional_test]
    fn test_set_up_extensions_update_maintain_ab_update() {
        // Test scenario where an old sysext and a new sysext match on ID, and
        // an update is NOT required (matching hashes).
        // Create servicing OS filesystem
        let loopback = NamedTempFile::new().unwrap();
        loopback.as_file().set_len(1024 * 1024).unwrap();
        mkfs::run(loopback.path(), MkfsFileSystemType::Ext4).unwrap();

        let old_ext_mount = Path::new("/mnt/tmpfs");
        fs::create_dir_all(old_ext_mount).unwrap();
        mount::mount(
            "tmpfs",
            old_ext_mount,
            MountFileSystemType::Tmpfs,
            &["size=1M".into()],
        )
        .unwrap();

        // Create old extension
        let old_ext = NamedTempFile::new_in(old_ext_mount).unwrap();
        let hash = create_test_extension_image(
            old_ext.path(),
            "my_ext",
            &ExtensionType::Sysext,
            "ID=_any\nSYSEXT_ID=my_ext",
        );

        let target_path = "/etc/extensions/updated_ext.raw";
        let subsystem = ExtensionsSubsystem {
            extensions: vec![ExtensionData {
                id: "my_ext".to_string(),
                name: "my_ext".to_string(),
                sha384: hash.clone(),
                path: PathBuf::from(target_path),
                temp_path: old_ext.path().to_path_buf(), // Sysext exists on servicing OS, so temp_path should point to this file.
                ext_type: ExtensionType::Sysext,
            }],
            extensions_old: vec![ExtensionData {
                id: "my_ext".to_string(),
                name: "my_ext".to_string(),
                sha384: hash,
                path: old_ext.path().to_path_buf(),
                temp_path: old_ext.path().to_path_buf(),
                ext_type: ExtensionType::Sysext,
            }],
            ..Default::default()
        };

        let mount_path = TempDir::new().unwrap();
        // Create necessary directories
        subsystem.create_directories(mount_path.path()).unwrap();
        // Run set_up_extensions
        subsystem
            .set_up_extensions(mount_path.path(), ServicingType::AbUpdate)
            .unwrap();

        // Verify old extension was not removed. Since old extension exists on a
        // separate filesystem, rename should fail and file should be copied.
        assert!(
            old_ext.path().exists(),
            "Old extension should not be removed from the servicing OS"
        );

        // Verify old extension was copied to target OS
        assert!(
            path::join_relative(mount_path.path(), target_path).exists(),
            "Old extension should be copied"
        );
        assert_eq!(
            fs::read(&old_ext).unwrap(),
            fs::read(path::join_relative(mount_path.path(), target_path)).unwrap(),
            "Old extension should match version on target OS"
        );

        // Clean-up
        drop(old_ext);
        mount::umount(old_ext_mount, true).unwrap();
    }

    #[functional_test]
    fn test_set_up_extensions_update_maintain_runtime_update() {
        // Test scenario where an old sysext and a new sysext match on ID, and
        // an update is NOT required (matching hashes), on a runtime update.
        // Create old extension.
        let old_ext_dir = TempDir::new().unwrap();
        let old_ext = NamedTempFile::new_in(&old_ext_dir).unwrap();
        let hash = create_test_extension_image(
            old_ext.path(),
            "my_ext",
            &ExtensionType::Sysext,
            "ID=_any\nSYSEXT_ID=my_ext",
        );

        let target_path = "/etc/extensions/updated_ext.raw";
        let subsystem = ExtensionsSubsystem {
            extensions: vec![ExtensionData {
                id: "my_ext".to_string(),
                name: "my_ext".to_string(),
                sha384: hash.clone(),
                path: PathBuf::from(target_path),
                temp_path: old_ext.path().to_path_buf(), // Sysext exists on servicing OS, so temp_path should point to this file.
                ext_type: ExtensionType::Sysext,
            }],
            extensions_old: vec![ExtensionData {
                id: "my_ext".to_string(),
                name: "my_ext".to_string(),
                sha384: hash,
                path: old_ext.path().to_path_buf(),
                temp_path: old_ext.path().to_path_buf(),
                ext_type: ExtensionType::Sysext,
            }],
            ..Default::default()
        };

        let mount_path = TempDir::new().unwrap();
        // Create necessary directories
        subsystem.create_directories(mount_path.path()).unwrap();
        // Run set_up_extensions
        subsystem
            .set_up_extensions(mount_path.path(), ServicingType::RuntimeUpdate)
            .unwrap();

        // Verify old extension was removed. Since servicing OS == target OS,
        // rename should succeed.
        assert!(
            !old_ext.path().exists(),
            "Old extension should be removed from the servicing OS"
        );

        // Verify extension exists at new location on target OS
        assert!(
            path::join_relative(mount_path.path(), target_path).exists(),
            "Extension should be at new location"
        );
    }
}
