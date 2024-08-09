//! Validation errors for the host configuration.

use serde::{Deserialize, Serialize};

/// Identifies errors detected during static validation of the host configuration, i.e. errors that
/// can be detected without applying the configuration to the host.
#[derive(thiserror::Error, Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum HostConfigurationStaticValidationError {
    #[error("Additional file '{additional_file}' has both content and path, but only one must be specified")]
    AdditionalFileBothContentAndPath { additional_file: String },

    #[error("Additional file '{additional_file}' has invalid permissions '{permissions}'")]
    AdditionalFileInvalidPermissions {
        additional_file: String,
        permissions: String,
    },

    #[error(
        "Additional file '{additional_file}' has no content or path, but one must be specified"
    )]
    AdditionalFileNoContentOrPath { additional_file: String },

    #[error("Datastore path '{datastore_path}' cannot be in A/B update volume '{volume_id}'")]
    DatastorePathInABUpdateVolume {
        datastore_path: String,
        volume_id: String,
    },

    #[error("Datastore file has extension '{received}', but must have '{expected}'")]
    DatastorePathInvalidExtension { received: String, expected: String },

    #[error("Datastore path '{datastore_path}' must be in a known volume")]
    DatastorePathNotInKnownVolume { datastore_path: String },

    #[error("Host configuration contains duplicate usernames '{username}', but usernames must be unique")]
    DuplicateUsernames { username: String },

    #[error("Underlying device of encrypted volume '{encrypted_volume}' must be a partition or a software RAID array")]
    EncryptedVolumeNotPartitionOrRaid { encrypted_volume: String },

    #[error("Failed to find expected mount point '{mount_point_path}'")]
    ExpectedMountPointNotFound { mount_point_path: String },

    #[error(transparent)]
    InvalidBlockDeviceGraph(
        #[from] super::storage::blkdev_graph::error::BlockDeviceGraphBuildError,
    ),

    #[error("Encryption recovery key URL '{url}' has invalid scheme '{scheme}'")]
    InvalidEncryptionRecoveryKeyUrlScheme { url: String, scheme: String },

    #[error("Interface name '{name}' is invalid")]
    InvalidInterfaceName { name: String },

    #[error("Netplan version '{version}' is invalid, must always be '2'")]
    InvalidNetplanVersion { version: u8 },

    #[error("Mount point '{mount_point_path}' must be backed by A/B update volume pair")]
    MountPointNotBackedByAbUpdateVolumePair { mount_point_path: String },

    #[error("Mount point '{mount_point_path}' must be backed by a block device")]
    MountPointNotBackedByBlockDevice { mount_point_path: String },

    #[error("Mount point '{mount_point_path}' must be backed by an image")]
    MountPointNotBackedByImage { mount_point_path: String },

    #[error(
        "Overlay '{overlay_path}' cannot be on volume '{mount_point_path}' as it is read-only"
    )]
    OverlayOnReadOnlyVolume {
        overlay_path: String,
        mount_point_path: String,
    },

    #[error("Overlay '{overlay_path}' cannot be on volume '{mount_point_path}' as it is verity-protected")]
    OverlayOnVerityProtectedVolume {
        overlay_path: String,
        mount_point_path: String,
    },

    #[error("Path '{path}' must be absolute")]
    PathNotAbsolute { path: String },

    #[error("Root verity device name '{device_name}' is invalid, must be 'root'")]
    RootVerityDeviceNameInvalid { device_name: String },

    #[error("Script '{script_name}' has both content and path, but only one must be specified")]
    ScriptBothContentAndPath { script_name: String },

    #[error("Script '{script_name}' has no content or path, but one must be specified")]
    ScriptNoContentOrPath { script_name: String },

    #[error("Cannot request self-upgrade of Trident when a read-only verity filesystem is mounted at '/'")]
    SelfUpgradeOnReadOnlyRootVerityFs,

    #[error("Netplan renderer '{renderer}' is not supported")]
    UnsupportedNetplanRenderer { renderer: String },

    #[error("Only root verity device is supported, but other verity devices were requested")]
    UnsupportedVerityDevices,

    #[error("Verity device '{device_name}' is mounted read-write at '{mount_point_path}', but must be mounted read-only")]
    VerityDeviceMountedReadWrite {
        device_name: String,
        mount_point_path: String,
    },
}

/// Identifies errors detected during dynamic validation of the host configuration, i.e. errors
/// that can only be detected by applying the configuration to the host.
#[derive(thiserror::Error, Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum HostConfigurationDynamicValidationError {
    #[error("Cannot adopt partitions on disk '{disk_id}', as it does not use GPT partitioning")]
    AdoptPartitionsOnNonGptPartitionedDisk { disk_id: String },

    #[error("Datastore path was changed from '{current}' to '{new}', but can only be changed during clean install")]
    DatastorePathChanged { current: String, new: String },

    #[error(
        "Disk definitions '{disk1}' and '{disk2}' refer to the same device '{device}', but must be unique"
    )]
    DiskDefinitionsReferToSameDevice {
        disk1: String,
        disk2: String,
        device: String,
    },

    #[error("Images and host configuration have incompatible dm-verity configuration")]
    DmVerityMisconfiguration,

    #[error("Encryption recovery key file '{key_file}' must not be empty")]
    EncryptionKeyEmpty { key_file: String },

    #[error("Recovery key file '{key_file}' must not be readable or writable by group or others but has permissions 0o{permissions:03o}")]
    EncryptionKeyInvalidPermissions { key_file: String, permissions: u32 },

    #[error("Encryption recovery key file '{key_file}' must be a regular file")]
    EncryptionKeyNotRegularFile { key_file: String },

    #[error("Failed to get block device information for disk '{disk_id}' that requires partition adoption")]
    GetBlockDeviceInfoForDisk { disk_id: String },

    #[error("Failed to get metadata for encryption recovery key file '{key_file}'")]
    GetEncryptionKeyMetadata { key_file: String },

    #[error("Cannot update image on standalone block device '{device_id}' during A/B update")]
    ImageUpdateOnStandaloneBlockDevice { device_id: String },

    #[error("Encryption recovery key file has invalid path '{path}'")]
    InvalidEncryptionKeyFilePath { path: String },

    #[error("Disk '{name}' refers to device '{device}', but its device must be under '/dev'")]
    InvalidDiskBlockDevicePath { name: String, device: String },

    #[error("Disk '{disk_id}' has invalid path '{disk_path}'")]
    InvalidDiskPath { disk_id: String, disk_path: String },

    #[error("Script '{name}' has invalid path '{path}'")]
    InvalidScriptPath { name: String, path: String },

    #[error("Failed to load additional file '{name}' to be placed at '{path}'")]
    LoadAdditionalFile { name: String, path: String },

    #[error("Failed to load script '{name}' at '{path}'")]
    LoadScript { name: String, path: String },
}
