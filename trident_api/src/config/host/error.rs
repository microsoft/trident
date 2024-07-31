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

    #[error(
        "Image update must not be requested for standalone block device with id '{0}' during A/B update"
    )]
    AbUpdateNotAllowedForStandaloneBlockDevice(String),

    #[error("Path '{disk_path}' of disk with id '{disk_id}' cannot be found in the system")]
    InvalidDiskPath { disk_path: String, disk_id: String },

    #[error("Failed to get block device information for disk with id '{0}' that requires partition adoption")]
    DiskForPartitionAdoptionInfoFailed(String),

    #[error("File for script '{name}' not found on host at '{path}'")]
    ScriptNotFound { name: String, path: String },

    #[error("Failed to load script '{name}' at '{path}'")]
    ScriptLoadFailed { name: String, path: String },

    #[error("Failed to load additional file '{name}' to be placed at '{path}'")]
    AdditionalFileLoadFailed { name: String, path: String },

    #[error("Encryption recovery key file '{0}' not found")]
    EncryptionKeyNotFound(String),

    #[error("Failed to get metadata for encryption recovery key file '{0}'")]
    EncryptionKeyMetadataFailed(String),

    #[error("Encryption recovery key file '{0}' must not be empty")]
    EncryptionKeyEmpty(String),

    #[error("Encryption recovery key file '{0}' must be a regular file")]
    EncryptionKeyNotRegularFile(String),

    #[error("Recovery key file '{key_file}' must not be readable or writable by group or others but has permissions 0o{permissions:03o}")]
    EncryptionKeyInvalidPermissions { key_file: String, permissions: u32 },

    #[error("Partitions are being adopted on disk '{0}', but it is not using GPT partitioning")]
    AdoptionOnNonGptPartitionedDisk(String),

    #[error("Uncategorized error: {0}")]
    Other(String),
}
