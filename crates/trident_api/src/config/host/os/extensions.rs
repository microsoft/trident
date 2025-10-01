use std::path::PathBuf;

#[cfg(feature = "schemars")]
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::{
    config::HostConfigurationStaticValidationError,
    constants::{VALID_CONFEXT_DIRECTORIES, VALID_SYSEXT_DIRECTORIES},
    primitives::hash::Sha384Hash,
};

/// Data about an extension image (sysext or confext) to merge onto the target OS.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct Extension {
    /// The path to the extension image file.
    ///
    /// URLs may have one of the following four schemes: `http://`, `https://`, `file://`, or
    /// `oci://`. Extension image files stored in OCI registries must allow for
    /// anonymous pulls.
    pub url: Url,

    /// The Sha384 of the entire extension image file.
    pub sha384: Sha384Hash,

    /// The absolute path of the extension image in the target OS.
    pub location: Option<PathBuf>,
}

impl Extension {
    pub fn validate(&self) -> Result<(), HostConfigurationStaticValidationError> {
        // Ensure that the location, if given, is a valid location for the
        // extension image to be placed
        let Some(location) = &self.location else {
            return Ok(());
        };

        // Check that the directory is valid
        let Some(provided_dir) = location.parent() else {
            return Err(
                HostConfigurationStaticValidationError::ExtensionImageInvalidPath {
                    location: location.display().to_string(),
                    message: "Could not retrieve directory path".to_string(),
                },
            );
        };
        if !VALID_CONFEXT_DIRECTORIES
            .iter()
            .chain(VALID_SYSEXT_DIRECTORIES.iter())
            .any(|valid_dir| provided_dir == PathBuf::from(valid_dir))
        {
            return Err(
                HostConfigurationStaticValidationError::ExtensionImageInvalidDirectory {
                    location: location.display().to_string(),
                },
            );
        }

        // File must end in *.raw
        let Some(filename) = location.file_name().and_then(|f| f.to_str()) else {
            return Err(
                HostConfigurationStaticValidationError::ExtensionImageInvalidPath {
                    location: location.display().to_string(),
                    message: "Could not retrieve file name".to_string(),
                },
            );
        };
        if !filename.ends_with(".raw") {
            return Err(
                HostConfigurationStaticValidationError::ExtensionImageInvalidFileExtension {
                    location: location.display().to_string(),
                },
            );
        }

        Ok(())
    }
}
