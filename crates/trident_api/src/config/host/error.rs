//! Validation errors for Host Configuration.

use serde::{Deserialize, Serialize};

use crate::{
    constants::VAR_TMP_PATH,
    error::{InvalidInputError, TridentError},
};

use super::storage::storage_graph::error::StorageGraphBuildError;

/// Identifies errors detected during static validation of the Host Configuration, i.e. errors that
/// can be detected without applying the configuration to the host.
#[derive(thiserror::Error, Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum HostConfigurationStaticValidationError {
    #[error("Additional file '{additional_file}' has both content and source, but only one must be specified")]
    AdditionalFileBothContentAndSource { additional_file: String },

    #[error("Additional file '{additional_file}' has invalid permissions '{permissions}'")]
    AdditionalFileInvalidPermissions {
        additional_file: String,
        permissions: String,
    },

    #[error(
        "Additional file '{additional_file}' has no content or source, but one must be specified"
    )]
    AdditionalFileNoContentOrSource { additional_file: String },

    #[error("Datastore path '{datastore_path}' cannot be in A/B update volume '{volume_id}'")]
    DatastorePathInABUpdateVolume {
        datastore_path: String,
        volume_id: String,
    },

    #[error("Datastore file has extension '{received}', but must have '{expected}'")]
    DatastorePathInvalidExtension { received: String, expected: String },

    #[error("Datastore path '{datastore_path}' must be in a known volume")]
    DatastorePathNotInKnownVolume { datastore_path: String },

    #[error("Host Configuration contains extension images with duplicate hashes '{hash}', but extension images must be unique")]
    DuplicateExtensionImage { hash: String },

    #[error("Host Configuration contains extension images with duplicate paths '{path}', but extension images must have unique paths")]
    DuplicateExtensionImagePath { path: String },

    #[error("Host Configuration contains duplicate usernames '{username}', but usernames must be unique")]
    DuplicateUsernames { username: String },

    #[error("Underlying device of encrypted volume '{encrypted_volume}' must be a partition or a software RAID array")]
    EncryptedVolumeNotPartitionOrRaid { encrypted_volume: String },

    #[error("Failed to find expected mount point '{mount_point_path}'")]
    ExpectedMountPointNotFound { mount_point_path: String },

    #[error(
        "Extension image path '{path}' is invalid; valid directories to place this extension image are: {valid_directories}"
    )]
    ExtensionImageInvalidDirectory {
        path: String,
        valid_directories: String,
    },

    #[error(
        "Extension image path '{path}' is invalid; must be an absolute path to a file whose \
        filename must end with '.raw'"
    )]
    ExtensionImageInvalidFileExtension { path: String },

    #[error(
        "The Host Configuration is using both an image and partition images, these APIs are \
        mutually exclusive"
    )]
    ImageApiMixed,

    #[error(transparent)]
    InvalidStorageGraph(#[from] StorageGraphBuildError),

    #[error("Encryption recovery key URL '{url}' has invalid scheme '{scheme}'")]
    InvalidEncryptionRecoveryKeyUrlScheme { url: String, scheme: String },

    #[error("Interface name '{name}' is invalid")]
    InvalidInterfaceName { name: String },

    #[error("Netplan version '{version}' is invalid, must always be '2'")]
    InvalidNetplanVersion { version: u8 },

    #[error("Invalid URL provided '{url}': '{explanation}'")]
    InvalidSourceUrl { url: String, explanation: String },

    #[error("Mount point '{mount_point_path}' must be backed by A/B update volume pair")]
    MountPointNotBackedByAbUpdateVolumePair { mount_point_path: String },

    #[error("Mount point '{mount_point_path}' must be backed by a block device")]
    MountPointNotBackedByBlockDevice { mount_point_path: String },

    #[error("Mount point '{mount_point_path}' must be backed by an image")]
    MountPointNotBackedByImage { mount_point_path: String },

    #[error(
        "Directory '{VAR_TMP_PATH}' must be on a read-write volume, but is on a read-only \
        volume mounted at '{mount_point_path}'"
    )]
    VarTmpOnReadOnlyVolume { mount_point_path: String },

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

    #[error("Verity device name '{device_name}' is invalid, must be '{expected}'")]
    VerityDeviceNameInvalid {
        device_name: String,
        expected: String,
    },

    #[error("Cannot request self-upgrade of Trident when a read-only verity filesystem is mounted at '/'")]
    SelfUpgradeOnReadOnlyRootVerityFs,

    #[error(
        "List of PCRs in encryption config contains unsupported PCRs '{pcrs}'.\n
        Only PCRs 4, 7, and 11 are supported"
    )]
    UnsupportedEncryptionPcrs { pcrs: String },

    #[error("Netplan renderer '{renderer}' is not supported")]
    UnsupportedNetplanRenderer { renderer: String },

    #[error("Unsupported URL scheme provided '{url_scheme}', must be 'file', 'http', or 'https'")]
    UnsupportedSourceUrlScheme { url_scheme: String },

    #[error("Only one of root or usr-verity devices is supported, but other verity devices were requested")]
    UnsupportedVerityDevices,

    #[error("In order to use usr-verity, UKI support must be enabled")]
    UsrVerityRequiresUkiSupport,

    #[error("Verity device '{device_name}' must define a mount point")]
    VerityFilesystemWithoutMountPoint { device_name: String },

    #[error(
        "Verity device '{device_name}' is mounted read-write at '{mount_point_path}', but must \
        be mounted read-only"
    )]
    VerityDeviceMountedReadWrite {
        device_name: String,
        mount_point_path: String,
    },
}

impl From<HostConfigurationStaticValidationError> for TridentError {
    fn from(inner: HostConfigurationStaticValidationError) -> Self {
        TridentError::new(InvalidInputError::InvalidHostConfigurationStatic { inner })
    }
}

/// Identifies errors detected during dynamic validation of Host Configuration, i.e. errors that
/// can only be detected by applying the configuration to the host.
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

    #[error("Host Configuration contains partition images, but an OS image must be used if one was previously deployed")]
    DeployPartitionImagesAfterOsImage,

    #[error("Host Configuration contains OS image, but partition images must be used if they were previously deployed")]
    DeployOsImageAfterPartitionImages,

    #[error("Images and Host Configuration have incompatible dm-verity configuration")]
    DmVerityMisconfiguration,

    #[error("Encryption recovery key file '{key_file}' must not be empty")]
    EncryptionKeyEmpty { key_file: String },

    #[error("Recovery key file '{key_file}' must not be readable or writable by group or others but has permissions 0o{permissions:03o}")]
    EncryptionKeyInvalidPermissions { key_file: String, permissions: u32 },

    #[error("Encryption recovery key file '{key_file}' must be a regular file")]
    EncryptionKeyNotRegularFile { key_file: String },

    #[error(
        "Since update image is a grub image, list of PCRs in encryption config contains invalid PCRs: '{pcrs}'. \
        Only PCR 7 is valid for grub images"
    )]
    InvalidEncryptionPcrsForGrubImage { pcrs: String },

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

    #[error(
        "SELinux is not supported with root-verity and grub. SELinux is set to '{selinux_mode}', \
        but should be set to 'disabled'"
    )]
    RootVerityAndSelinuxUnsupported { selinux_mode: String },

    #[error(
        "Since Trident is running in a container, PCR 7 cannot be used for encryption \
        in the target UKI OS"
    )]
    Pcr7EncryptionForUkiWhenRunningInContainer,

    #[error(
        "Since Secure Boot is disabled, PCR 7 cannot be used for encryption \
        in the target UKI OS"
    )]
    Pcr7EncryptionForUkiWhenSecureBootDisabled,

    #[error("Cannot modify storage configuration during update")]
    StorageConfigurationChanged,
}
