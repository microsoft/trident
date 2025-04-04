use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[cfg(feature = "schemars")]
use schemars::JsonSchema;

use crate::{
    config::HarpoonConfig,
    constants::{DATASTORE_FILE_EXTENSION, TRIDENT_DATASTORE_PATH_DEFAULT},
    is_default,
};

use super::error::HostConfigurationStaticValidationError;

/// The Trident Management configuration controls the installation of the
/// Trident agent onto the runtime OS.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct Trident {
    /// When set to `true`, prevents Trident from being enabled on the runtime OS.
    /// In that case, the remaining fields are ignored.
    #[serde(default)]
    pub disable: bool,

    /// (FOR DEBUGGING ONLY) a boolean flag that indicates whether Trident should
    /// upgrade itself. If set to `true`, Trident will replicate itself into the
    /// runtime OS prior to rebooting. This is useful during development to
    /// ensure that the matching version of Trident is used. Defaults to `false`.
    #[serde(default)]
    pub self_upgrade: bool,

    /// Whether Trident should start a gRPC server to listen for commands when the runtime OS boots.
    /// Defaults to `false`.
    #[serde(default, skip_serializing_if = "is_default")]
    pub enable_grpc: bool,

    /// Describes where to place the datastore Trident will use to store its state.
    /// Defaults to `/var/lib/trident/datastore.sqlite`. Needs to end with
    /// `.sqlite`, cannot be an existing file and cannot reside on a read-only
    /// filesystem or A/B volume.
    #[serde(
        default = "Trident::default_datastore_path",
        skip_serializing_if = "Trident::is_default_datastore_path"
    )]
    pub datastore_path: PathBuf,

    /// URL to reach out to when runtime OS networking is up, so Trident can report
    /// its status. If not specified, the value from the Trident configuration will
    /// be used. This is useful for debugging and monitoring purposes, say by an
    /// orchestrator.
    pub phonehome: Option<String>,

    /// Optional URL to stream logs to. TODO: document the interface.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logstream: Option<String>,

    /// Optional Harpoon configuration.
    ///
    /// Harpoon is an Omaha client that Trident can use to check for updated
    /// host configuration files.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schemars", schemars(skip))]
    pub harpoon: Option<HarpoonConfig>,
}

impl Default for Trident {
    fn default() -> Self {
        Self {
            disable: Default::default(),
            self_upgrade: Default::default(),
            enable_grpc: Default::default(),
            datastore_path: Trident::default_datastore_path(),
            phonehome: Default::default(),
            logstream: Default::default(),
            harpoon: Default::default(),
        }
    }
}

impl Trident {
    /// Returns the default Trident datastore path.
    pub(crate) fn default_datastore_path() -> PathBuf {
        PathBuf::from(TRIDENT_DATASTORE_PATH_DEFAULT)
    }

    fn is_default_datastore_path(other: &Path) -> bool {
        other == Path::new(TRIDENT_DATASTORE_PATH_DEFAULT)
    }

    /// Validate the Trident Management configuration.
    pub fn validate(&self) -> Result<(), HostConfigurationStaticValidationError> {
        // Nothing to do if Trident is disabled on the runtime OS.
        if self.disable {
            return Ok(());
        }

        if self
            .datastore_path
            .extension()
            .and_then(|e| (e == DATASTORE_FILE_EXTENSION).then_some(()))
            .is_none()
        {
            return Err(
                HostConfigurationStaticValidationError::DatastorePathInvalidExtension {
                    received: self
                        .datastore_path
                        .extension()
                        .map(|e| e.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "<none>".into()),
                    expected: DATASTORE_FILE_EXTENSION.into(),
                },
            );
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_datastore_path() {
        let mut trident = Trident {
            datastore_path: PathBuf::from("/var/lib/trident/datastore.sqlite"),
            ..Default::default()
        };
        trident.validate().unwrap();

        trident.datastore_path = PathBuf::from("/var/lib/trident/datastore");
        assert_eq!(
            trident.validate().unwrap_err(),
            HostConfigurationStaticValidationError::DatastorePathInvalidExtension {
                received: "<none>".into(),
                expected: DATASTORE_FILE_EXTENSION.into(),
            }
        );

        trident.datastore_path = PathBuf::from("/var/lib/trident/datastore.docx");
        assert_eq!(
            trident.validate().unwrap_err(),
            HostConfigurationStaticValidationError::DatastorePathInvalidExtension {
                received: "docx".into(),
                expected: DATASTORE_FILE_EXTENSION.into(),
            }
        );
    }
}
