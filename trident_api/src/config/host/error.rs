//! Validation errors for the host configuration.

use serde::{Deserialize, Serialize};

#[derive(thiserror::Error, Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum InvalidHostConfigurationError {
    #[error("Failed to parse host configuration")]
    FailedToParse,

    #[error("Duplicate user name: {0}")]
    DuplicateUsernames(String),

    #[error("Script '{0}' has no content or path")]
    ScriptHasNoContentOrPath(String),

    #[error("Script '{0}' has both content and path")]
    ScriptHasBothContentAndPath(String),

    #[error("Encryption recovery key URL '{url}' has an invalid scheme '{scheme}'")]
    InvalidEncryptionRecoveryKeyUrlScheme { url: String, scheme: String },

    #[error(transparent)]
    InvalidBlockDeviceGraph(
        #[from] super::storage::blkdev_graph::error::BlockDeviceGraphBuildError,
    ),

    #[error("Expected mount point '{mount_point_path}' not found")]
    ExpectedMountPointNotFound { mount_point_path: String },

    #[error("Mount point '{mount_point_path}' not backed by an image")]
    MountPointNotBackedByImage { mount_point_path: String },
}
