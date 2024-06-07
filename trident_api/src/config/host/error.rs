//! Validation errors for the host configuration.

use serde::{Deserialize, Serialize};

#[derive(thiserror::Error, Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum HostConfigurationStaticValidationError {
    #[error("Failed to parse host configuration")]
    FailedToParse,

    #[error("Duplicate user name: {0}")]
    DuplicateUsernames(String),

    #[error("Script '{0}' has no content or path")]
    ScriptHasNoContentOrPath(String),

    #[error("Script '{0}' has both content and path")]
    ScriptHasBothContentAndPath(String),

    #[error("Added file '{0}' has no content or path")]
    AdditionalFileHasNoContentOrPath(String),

    #[error("Added file '{0}' has both content and path")]
    AdditionalFileHasBothContentAndPath(String),

    #[error("Could not parse permissions '{0}' for added file '{1}'")]
    AdditionalFileInvalidPermissions(String, String),

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

    #[error("The interface name '{0}' is invalid")]
    InvalidInterfaceName(String),

    #[error("Invalid Netplan version. It should always be '2', but got '{0}'")]
    InvalidNetplanVersion(u8),

    #[error("Unsupported Netplan renderer: '{0}'")]
    UnsupportedNetplanRenderer(String),

    #[error("Only root verity device is supported, but additional verity devices were requested")]
    UnsupportedVerityDevices,

    #[error("Mount point '{mount_point_path}' not backed by A/B update volume pair")]
    MountPointNotBackedByAbUpdateVolumePair { mount_point_path: String },

    #[error("Root verity device name is invalid: '{device_name}', expected 'root'")]
    RootVerityDeviceNameInvalid { device_name: String },

    #[error("Overlay '{overlay_path}' is on a read-only volume '{mount_point_path}'")]
    OverlayOnReadOnlyVolume {
        mount_point_path: String,
        overlay_path: String,
    },

    #[error("Verity device '{device_name}' not mounted read-only at '{mount_point_path}'")]
    VerityDeviceReadWrite {
        device_name: String,
        mount_point_path: String,
    },

    #[error("Mount point '{mount_point_path}' is not backed by a block device")]
    MountPointNotBackedByBlockDevice { mount_point_path: String },

    #[error("Cannot self-upgrade Trident when a read-only verity filesystem is mounted at '/'")]
    SelfUpgradeOnReadOnlyRootVerityFsError,

    #[error("Datastore file extension should be '{expected}', but got '{got}'")]
    DatastorePathInvalidExtension { expected: String, got: String },

    #[error("Datastore path '{datastore_path}' is not in any known volume")]
    DatastorePathNotInAnyKnownVolume { datastore_path: String },

    #[error("Datastore path '{datastore_path}' is in an A/B update volume: '{volume_id}'")]
    DatastorePathInABVolume {
        datastore_path: String,
        volume_id: String,
    },

    #[error("Path '{path}' is not absolute path")]
    PathNotAbsolute { path: String },
}

#[derive(thiserror::Error, Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum HostConfigurationDynamicValidationError {
    #[error("Datastore path cannot be changed. Current: '{current}'. New: '{new}'")]
    ChangedDatastorePath { current: String, new: String },

    #[error("File for script '{name}' not found on host at '{path}'")]
    ScriptNotFound { name: String, path: String },

    #[error("The disk '{name}' refers to device '{device}' which is not under '/dev'")]
    BadBlockDevicePath { name: String, device: String },

    #[error(
        "Multiple disk definitions refer to the same device '{device}': '{disk1}' and '{disk2}'"
    )]
    DiskDefinitionsReferToSameDevice {
        disk1: String,
        disk2: String,
        device: String,
    },

    #[error("Encryption configuration is incorrect:\n{0}")]
    EncryptionIncorrect(String),

    #[error("Failed to parse host configuration")]
    ImagesIncorrect(String),

    #[error("Uncategorized error: {0}")]
    Other(String),
}

/// Temporary helper to convert existing code to the new error types.
impl From<anyhow::Error> for HostConfigurationDynamicValidationError {
    fn from(value: anyhow::Error) -> Self {
        Self::Other(format!("{:#}", value))
    }
}
