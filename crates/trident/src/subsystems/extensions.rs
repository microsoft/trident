use std::{
    fmt::Display,
    fs, io,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{bail, ensure, Context, Error};
use log::debug;
use tempfile::NamedTempFile;

use osutils::{dependencies::Dependency, osrelease::OsRelease};
use trident_api::{
    config::Extension,
    constants::internal_params::COSI_HTTP_CONNECTION_TIMEOUT_SECONDS,
    error::{InternalError, ReportError, TridentError},
    primitives::hash::Sha384Hash,
};

use crate::{
    engine::{EngineContext, Subsystem},
    io_utils::{
        file_reader::FileReader, hashing_reader::HashingReader384, image_streamer::stream_and_hash,
    },
};

/// Expected extension-release directory for sysexts
const SYSEXT_EXTENSION_RELEASE_DIRECTORY: &str = "usr/lib/extension-release.d/";
/// Expected extension-release directory for confexts
const CONFEXT_EXTENSION_RELEASE_DIRECTORY: &str = "etc/extension-release.d/";

/// Primary location for storing sysexts on the target OS
const DEFAULT_SYSEXT_DIRECTORY: &str = "var/lib/extensions/";
/// Primary location for storing confexts on the target OS
const DEFAULT_CONFEXT_DIRECTORY: &str = "var/lib/confexts/";

/// Temporary directory on target OS for downloading extension images
const EXTENSION_IMAGE_DOWNLOAD_DIRECTORY: &str = "var/lib/.extensions-staging/";

#[derive(Clone, Debug)]
pub struct ExtensionData {
    pub id: String,
    pub name: String,
    pub sha384: Sha384Hash,
    pub path: PathBuf,
    pub temp_path: Option<PathBuf>,
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

#[derive(Default, Debug)]
pub struct ExtensionsSubsystem {
    extensions: Vec<ExtensionData>,
    extensions_old: Vec<ExtensionData>,
}
impl Subsystem for ExtensionsSubsystem {
    fn name(&self) -> &'static str {
        "extensions"
    }

    fn validate_host_config(&self, _ctx: &EngineContext) -> Result<(), TridentError> {
        Ok(())
    }

    // Servicing OS
    fn prepare(&mut self, _ctx: &EngineContext) -> Result<(), TridentError> {
        Ok(())
    }

    // Servicing OS, with access to target OS
    fn provision(&mut self, ctx: &EngineContext, mount_path: &Path) -> Result<(), TridentError> {
        // Download new extension images. Mount and process all extension images.
        self.populate_extensions(ctx, mount_path)
            .structured(InternalError::PopulateExtensionImages("Failed".to_string()))?;

        // TODO: Copy extension images to their proper locations.
        Ok(())
    }

    fn configure(&mut self, _ctx: &EngineContext) -> Result<(), TridentError> {
        Ok(())
    }
}

impl ExtensionsSubsystem {
    fn populate_extensions(
        self: &mut ExtensionsSubsystem,
        ctx: &EngineContext,
        mount_path: &Path,
    ) -> Result<(), Error> {
        let timeout = match ctx
            .spec
            .internal_params
            .get_u64(COSI_HTTP_CONNECTION_TIMEOUT_SECONDS)
        {
            Some(Ok(timeout)) => Duration::from_secs(timeout),
            _ => Duration::from_secs(10), // Default timeout
        };

        let temporary_staging_dir = mount_path.join(EXTENSION_IMAGE_DOWNLOAD_DIRECTORY);
        if !temporary_staging_dir.exists() {
            fs::create_dir_all(&temporary_staging_dir).with_context(|| {
                format!("Failed to create dir '{EXTENSION_IMAGE_DOWNLOAD_DIRECTORY}")
            })?;
        };

        self.populate_extensions_inner(
            ctx,
            timeout,
            &temporary_staging_dir,
            ExtensionType::Sysext,
            true,
        )?;
        self.populate_extensions_inner(
            ctx,
            timeout,
            &temporary_staging_dir,
            ExtensionType::Sysext,
            false,
        )?;
        self.populate_extensions_inner(
            ctx,
            timeout,
            &temporary_staging_dir,
            ExtensionType::Confext,
            true,
        )?;
        self.populate_extensions_inner(
            ctx,
            timeout,
            &temporary_staging_dir,
            ExtensionType::Confext,
            false,
        )?;
        Ok(())
    }

    /// Appends to `self.extensions` or `self.extensions_old`. Takes in 4 arguments:
    /// - self: ExtensionsSubsystem.
    /// - ctx: EngineContext.
    /// - timeout: Time out on HTTP requests.
    /// - ext_type: ExtensionType.
    /// - new: Boolean indicating whether this function should populate
    ///   `self.extensions` or `self.extensions_old`. When populating
    ///   `self.extensions_old`, expect all extensions in the old Host Configuration
    ///   to be present on the servicing OS so we will not download any new images.
    fn populate_extensions_inner(
        self: &mut ExtensionsSubsystem,
        ctx: &EngineContext,
        timeout: Duration,
        temp_staging_dir: &Path,
        ext_type: ExtensionType,
        new: bool,
    ) -> Result<(), Error> {
        let hc_extensions = match (new, &ext_type) {
            (true, ExtensionType::Sysext) => &ctx.spec.os.sysexts, // Populate new sysexts
            (false, ExtensionType::Sysext) => &ctx.spec_old.os.sysexts, // Populate old sysexts
            (true, ExtensionType::Confext) => &ctx.spec.os.confexts, // Populate new confexts
            (false, ExtensionType::Confext) => &ctx.spec_old.os.confexts, // Populate old confexts
        };

        for ext in hc_extensions {
            let extension_file = if new {
                // First, check if this extension already exists on the system.
                if let Some(existing_file_path) = match &ext_type {
                    ExtensionType::Sysext => {
                        check_for_path_in_old_host_configuration(ext, &ctx.spec_old.os.sysexts)
                    }
                    ExtensionType::Confext => {
                        check_for_path_in_old_host_configuration(ext, &ctx.spec_old.os.confexts)
                    }
                } {
                    ensure!(
                        existing_file_path.exists(),
                        "Expected to find extension image from URL '{}' at path '{}', but path does not exist",
                        ext.url,
                        existing_file_path.display()
                    );
                    existing_file_path
                } else {
                    // The extension is new to the OS, so we need to download it.
                    // Create and persist a temporary file; get its path
                    let temp_file: PathBuf = NamedTempFile::new_in(temp_staging_dir)
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
                    let computed_sha384 = stream_and_hash(hash_reader, &temp_file)
                        .context("Failed to read and write")?;

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
                // Ensure that file exists
                ensure!(
                    path.exists(),
                    "Expected to find extension image from URL '{}' at path '{}', but path does not exist",
                    ext.url,
                    path.display()
                );
                path
            };

            // Create temporary mountpoint, which will be used to read the extension-release file
            let temp_mp = tempfile::tempdir()?;

            // Attach a device and mount the extension
            let device_path = attach_device_and_mount(&extension_file, temp_mp.path())
                .context("Failed to mount")?;

            // Get extension release file
            let ext_data = read_extension_release(temp_mp.path(), &extension_file, ext, &ext_type)
                .context("Failed to get extension release information")?;

            if new {
                self.extensions.push(ext_data);
            } else {
                self.extensions_old.push(ext_data);
            }

            // Clean-Up: unmount and detach the device
            detach_device_and_unmount(device_path, temp_mp.path()).context("Failed to unmount")?;
        }

        Ok(())
    }
}

/// Helper function to identify if the extension exists in the old Host
/// Configuration, in which case we can reuse its path.
fn check_for_path_in_old_host_configuration(
    ext: &Extension,
    old_hc_extensions: &[Extension],
) -> Option<PathBuf> {
    old_hc_extensions
        .iter()
        // Extension must match on both URL and Sha384 hash
        .find(|old_ext| ext.url == old_ext.url && ext.sha384 == old_ext.sha384)?
        .path
        .clone()
}

/// Helper function to extract information from extension-release file
fn read_extension_release(
    mount_point: &Path,
    curr_path: &Path,
    ext: &Extension,
    ext_type: &ExtensionType,
) -> Result<ExtensionData, Error> {
    debug!(
        "Processing extension release file for extension image at '{}'",
        ext.url
    );

    let sysext_release_dir = mount_point.join(SYSEXT_EXTENSION_RELEASE_DIRECTORY);
    let confext_release_dir = mount_point.join(CONFEXT_EXTENSION_RELEASE_DIRECTORY);

    // Get extension release file
    let dir = match ext_type {
        ExtensionType::Sysext => fs::read_dir(&sysext_release_dir).with_context(|| format!("Failed to find extension release directory '{SYSEXT_EXTENSION_RELEASE_DIRECTORY}' in image at '{}'", ext.url))?,
        ExtensionType::Confext => fs::read_dir(&confext_release_dir).with_context(|| format!("Failed to find extension release directory '{CONFEXT_EXTENSION_RELEASE_DIRECTORY}' in image at '{}'", ext.url))?,
    }.map(|res| res.map(|e| e.path()))
    .collect::<Result<Vec<_>, io::Error>>()?;

    ensure!(
        dir.len() == 1,
        "Expected extension image to have exactly 1 extension-release file, found '{}'",
        dir.len()
    );

    // Read the extension release file
    let extension_release_file_path = &dir[0];
    let extension_release = OsRelease::read_file(extension_release_file_path)
        .context("Failed to read extension release file.")?;

    // Retrieve SYSEXT_ID or CONFEXT_ID field
    let extension_id = match ext_type {
        ExtensionType::Sysext => extension_release
            .sysext_id
            .context("Could not find SYSEXT_ID in extension release")?,
        ExtensionType::Confext => extension_release
            .confext_id
            .context("Could not find CONFEXT_ID in extension release")?,
    };
    let name = extension_release_file_path
        .file_name()
        .and_then(|s| s.to_str())
        .context("Failed to get file name as a valid UTF-8 string")?
        .strip_prefix("extension-release.")
        .context("Extension release filename must begin with 'extension-release.'")?
        .to_string();
    let path = match &ext.path {
        Some(path) => path.clone(),
        None => match ext_type {
            ExtensionType::Sysext => {
                PathBuf::from(DEFAULT_SYSEXT_DIRECTORY).join(format!("{name}.raw"))
            }
            ExtensionType::Confext => {
                PathBuf::from(DEFAULT_CONFEXT_DIRECTORY).join(format!("{name}.raw"))
            }
        },
    };

    Ok(ExtensionData {
        id: extension_id,
        name,
        sha384: ext.sha384.clone(),
        path,
        temp_path: Some(curr_path.to_path_buf()),
        ext_type: ext_type.clone(),
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
        .context("Failed to mount extension image")?;

    Ok(loop_device.to_string())
}

/// Helper function to unmount the extension image.
fn detach_device_and_unmount(device_path: String, mount_path: &Path) -> Result<(), Error> {
    Dependency::Umount
        .cmd()
        .arg(mount_path)
        .run_and_check()
        .context("Failed to unmount extension image")?;
    Dependency::Losetup
        .cmd()
        .arg("-d")
        .arg(device_path)
        .run_and_check()
        .context("Failed to detach loop device")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{fs::File, io::Write};

    use tempfile::{env::temp_dir, TempDir};
    use url::Url;

    fn create_extension(hash: Sha384Hash, path: Option<PathBuf>) -> Extension {
        Extension {
            url: Url::parse("https://example.com/test-extension").unwrap(),
            sha384: hash.clone(),
            path,
        }
    }

    #[test]
    fn test_read_extension_release_success() {
        let tempdir = TempDir::new().unwrap();
        let mount_point = tempdir.path();

        let sysext_release_dir = mount_point.join(SYSEXT_EXTENSION_RELEASE_DIRECTORY);
        fs::create_dir_all(&sysext_release_dir).unwrap();

        let mut extension_release_file =
            File::create(sysext_release_dir.join("extension-release.test_1.0.0")).unwrap();
        extension_release_file.write_all(b"ID=_any\nSYSEXT_ID=test\nSYSEXT_VERSION_ID=1.0.0\nSYSEXT_SCOPE=initrd system portable\nARCHITECTURE=x86-64").unwrap();

        // Create an Extension with no provided path
        let hash = Sha384Hash::from("a".repeat(96));
        let current_path = Path::new("/tmp/file");
        let extension = create_extension(hash.clone(), None);

        let extension_data = read_extension_release(
            mount_point,
            current_path,
            &extension,
            &ExtensionType::Sysext,
        )
        .unwrap();
        let expected_extension_data = ExtensionData {
            id: "test".to_string(),
            name: "test_1.0.0".to_string(),
            sha384: hash.clone(),
            path: PathBuf::from(DEFAULT_SYSEXT_DIRECTORY).join("test_1.0.0.raw"),
            temp_path: Some(PathBuf::from(current_path)),
            ext_type: ExtensionType::Sysext,
        };
        assert_eq!(extension_data.id, expected_extension_data.id);
        assert_eq!(extension_data.path, expected_extension_data.path);
        assert_eq!(extension_data.temp_path, expected_extension_data.temp_path);
        assert_eq!(extension_data.name, expected_extension_data.name);
        assert_eq!(extension_data.sha384, expected_extension_data.sha384);
        assert_eq!(extension_data.ext_type, expected_extension_data.ext_type);

        // Create an Extension with an intended path
        let final_path = PathBuf::from("/etc/extensions/test_1.0.0.raw");
        let extension_with_path = create_extension(hash.clone(), Some(final_path.clone()));

        let extension_data = read_extension_release(
            mount_point,
            current_path,
            &extension_with_path,
            &ExtensionType::Sysext,
        )
        .unwrap();
        let expected_extension_data = ExtensionData {
            id: "test".to_string(),
            name: "test_1.0.0".to_string(),
            sha384: hash,
            path: final_path,
            temp_path: Some(PathBuf::from(current_path)),
            ext_type: ExtensionType::Sysext,
        };
        assert_eq!(extension_data.id, expected_extension_data.id);
        assert_eq!(extension_data.path, expected_extension_data.path);
        assert_eq!(extension_data.temp_path, expected_extension_data.temp_path);
        assert_eq!(extension_data.name, expected_extension_data.name);
        assert_eq!(extension_data.sha384, expected_extension_data.sha384);
        assert_eq!(extension_data.ext_type, expected_extension_data.ext_type);
    }

    // Extension release directory does not exist
    #[test]
    fn test_read_extension_release_fails_no_file() {
        let tempdir = TempDir::new().unwrap();
        let mount_point = tempdir.path();

        let current_path = Path::new("/tmp/file");
        let extension = create_extension(Sha384Hash::from("a".repeat(96)), None);

        let result = read_extension_release(
            mount_point,
            current_path,
            &extension,
            &ExtensionType::Sysext,
        );
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            format!("Failed to find extension release directory '{SYSEXT_EXTENSION_RELEASE_DIRECTORY}' in image at 'https://example.com/test-extension'")
        );

        let result = read_extension_release(
            mount_point,
            current_path,
            &extension,
            &ExtensionType::Confext,
        );
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            format!("Failed to find extension release directory '{CONFEXT_EXTENSION_RELEASE_DIRECTORY}' in image at 'https://example.com/test-extension'")
        );
    }

    // There is not exactly one extension release file in the expected directory
    #[test]
    fn test_read_extension_release_fails_multiple_files() {
        let tempdir = TempDir::new().unwrap();
        let mount_point = tempdir.path();

        let sysext_release_dir = mount_point.join(SYSEXT_EXTENSION_RELEASE_DIRECTORY);
        fs::create_dir_all(&sysext_release_dir).unwrap();

        let current_path = Path::new("/tmp/file");
        let extension = create_extension(Sha384Hash::from("a".repeat(96)), None);

        // No extension release file exists.
        let result = read_extension_release(
            mount_point,
            current_path,
            &extension,
            &ExtensionType::Sysext,
        );
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            "Expected extension image to have exactly 1 extension-release file, found '0'"
        );

        // Create two extension release files.
        File::create(sysext_release_dir.join("extension-release.test1")).unwrap();
        File::create(sysext_release_dir.join("extension-release.test2")).unwrap();

        // Too many extension release files exist.
        let result = read_extension_release(
            mount_point,
            current_path,
            &extension,
            &ExtensionType::Sysext,
        );
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            "Expected extension image to have exactly 1 extension-release file, found '2'"
        );
    }

    // Extension release file is missing the SYSEXT_ID field
    #[test]
    fn test_read_extension_release_fails_missing_field() {
        let tempdir = TempDir::new().unwrap();
        let mount_point = tempdir.path();

        let sysext_release_dir = mount_point.join(SYSEXT_EXTENSION_RELEASE_DIRECTORY);
        fs::create_dir_all(&sysext_release_dir).unwrap();

        // Create a file with valid content but missing the SYSEXT_ID field.
        let mut file = File::create(sysext_release_dir.join("extension-release.test")).unwrap();
        file.write_all(b"ID=_any\nSYSEXT_VERSION_ID=1.0.0").unwrap();

        let current_path = Path::new("/tmp/file");
        let extension = create_extension(Sha384Hash::from("a".repeat(96)), None);

        let result = read_extension_release(
            mount_point,
            current_path,
            &extension,
            &ExtensionType::Sysext,
        );
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            "Could not find SYSEXT_ID in extension release"
        );
    }

    // Extension release file has an invalid name
    #[test]
    fn test_read_extension_release_fails_invalid_filename() {
        let tempdir = TempDir::new().unwrap();
        let mount_point = tempdir.path();

        let sysext_release_dir = mount_point.join(SYSEXT_EXTENSION_RELEASE_DIRECTORY);
        fs::create_dir_all(&sysext_release_dir).unwrap();

        // Create a file with a name that doesn't contain "extension-release."
        let mut file = File::create(sysext_release_dir.join("my-release-file")).unwrap();
        file.write_all(b"SYSEXT_ID=test").unwrap();

        let hash = Sha384Hash::from("a".repeat(96));
        let current_path = Path::new("/tmp/file");
        let extension = create_extension(hash, None);

        let err = read_extension_release(
            mount_point,
            current_path,
            &extension,
            &ExtensionType::Sysext,
        )
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "Extension release filename must begin with 'extension-release.'"
        );
    }

    #[test]
    fn test_populate_extensions_empty() {
        // Test with no extensions
        let mut subsystem = ExtensionsSubsystem::default();
        let ctx = EngineContext::default();
        subsystem.populate_extensions(&ctx, &temp_dir()).unwrap();

        assert!(
            subsystem.extensions.is_empty(),
            "ExtensionsSubsystem extensions should be empty when there are no extensions in the Host Configuration"
        );
        assert!(
            subsystem.extensions_old.is_empty(),
            "ExtensionsSubsystem extensions_old should be empty when there are no extensions in the old Host Configuration"
        );
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;

    use sha2::{Digest, Sha384};
    use tempfile::{env::temp_dir, TempDir};
    use url::Url;

    use pytest_gen::functional_test;

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
                "5M",
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
        let release_dir = mount_point.path().join(release_subdir);
        fs::create_dir_all(&release_dir).unwrap();

        let release_file_path = release_dir.join(format!("extension-release.{ext_name}"));
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
        let mut subsystem = ExtensionsSubsystem::default();
        subsystem.populate_extensions(&ctx, &temp_dir()).unwrap();

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
        let mut subsystem = ExtensionsSubsystem::default();
        subsystem.populate_extensions(&ctx, &temp_dir()).unwrap();

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
        let mut subsystem = ExtensionsSubsystem::default();
        let error = subsystem
            .populate_extensions(&ctx, &temp_dir())
            .unwrap_err()
            .to_string();

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
        let mut subsystem = ExtensionsSubsystem::default();
        let error = subsystem
            .populate_extensions(&ctx, &temp_dir())
            .unwrap_err()
            .to_string();

        assert_eq!(error, format!("Expected to find extension image from URL '{ext_url}' at path '{}', but path does not exist", ext_path.display()));
    }
}
