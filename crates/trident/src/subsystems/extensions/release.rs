use std::{
    fs, io,
    path::{Path, PathBuf},
};

use anyhow::{ensure, Context, Error};
use log::debug;

use osutils::osrelease::ExtensionRelease;
use trident_api::config::Extension;

use crate::subsystems::extensions::{
    ExtensionData, ExtensionType, CONFEXT_EXTENSION_RELEASE_DIRECTORY, DEFAULT_CONFEXT_DIRECTORY,
    DEFAULT_SYSEXT_DIRECTORY, SYSEXT_EXTENSION_RELEASE_DIRECTORY,
};

/// Helper function to extract information from extension-release file
pub(crate) fn read_extension_release(
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
    let extension_release = ExtensionRelease::read_file(extension_release_file_path)
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

#[cfg(test)]
mod tests {
    use super::*;

    use std::{fs::File, io::Write};

    use tempfile::TempDir;
    use trident_api::primitives::hash::Sha384Hash;
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
}
