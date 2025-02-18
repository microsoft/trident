use std::{
    borrow::Cow,
    fmt::{Debug, Write},
    panic::Location,
};

use serde::{ser::SerializeStruct, Deserialize, Serialize};
use strum_macros::IntoStaticStr;
use url::Url;

use crate::{
    config::{HostConfigurationDynamicValidationError, HostConfigurationStaticValidationError},
    status::ServicingState,
    storage_graph::error::StorageGraphBuildError,
};

/// Identifies errors that occur when the execution environment is misconfigured. This error type
/// can be attributed to the user, as it relates to the environment in which Trident is run.
#[derive(Debug, Eq, thiserror::Error, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ExecutionEnvironmentMisconfigurationError {
    #[error("Failed to run due to missing root privileges")]
    CheckRootPrivileges,

    #[error("Failed to find OS Modifier binary at '{binary_path}' required by '{config}'")]
    FindOSModifierBinary { binary_path: String, config: String },

    #[error("Failed to find required binary '{binary}'")]
    MissingBinary { binary: &'static str },
}

/// Identifies errors that occur when Trident fails to initialize.
#[derive(Debug, Eq, thiserror::Error, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum InitializationError {
    #[error("Safety check failed on clean install")]
    CleanInstallSafetyCheck,

    #[error("Failed to connect to logstream")]
    ConnectToLogstream,

    #[error("Failed to connect to tracestream")]
    ConnectToTracestream,

    #[error(transparent)]
    ContainerConfiguration {
        #[from]
        inner: ContainerConfigurationError,
    },

    #[error("Failed to load local Trident Host Status")]
    LoadHostStatus,

    #[error("Failed to parse Host Status")]
    ParseHostStatus,

    #[error("Failed to query for updates with Harpoon: {0}")]
    QueryForUpdates(String),

    #[error("Failed to read '/proc/cmdline'")]
    ReadCmdline,
}

/// Identifies errors that occur when the host is running from a docker container, but the system
/// is not configured correctly.
#[derive(Debug, Eq, thiserror::Error, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ContainerConfigurationError {
    #[error("Running from docker container, but {docker_env_var} environment variable is not set")]
    DockerEnvironmentVarCheck { docker_env_var: String },

    #[error("Running from docker container, but {host_root_path} is not mounted")]
    HostRootMountCheck { host_root_path: String },
}

/// Identifies errors that occur due to an internal bug or failure in Trident.
#[derive(Debug, Eq, thiserror::Error, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum InternalError {
    #[error("Failed to enqueue 'HostUpdate' command to the main Trident thread")]
    EnqueueHostUpdateCommand,

    #[error("Failed to get datastore path from local Trident config")]
    GetDatastorePathFromLocalTridentConfig,

    #[error("Failed to get the ESP partition information")]
    GetEspDeviceInfo,

    #[error(
        "Failed to find mount point in Host Configuration for filesystems sourced from an OS image"
    )]
    GetMountPointForOsImage,

    #[error("Failed to get root block device path")]
    GetRootBlockDevicePath,

    #[error("Internal error: {0}")]
    Internal(&'static str),

    #[error("Encountered a panic: {0}")]
    Panic(String),

    #[error("Failed to execute container-only logic as host is not running in a container")]
    RunInContainer,

    #[error("Failed to send Host Status")]
    SendHostStatus,

    #[error("Failed to serialize error")]
    SerializeError,

    #[error("Failed to serialize Host Status")]
    SerializeHostStatus,

    #[error("Failed to start tokio runtime")]
    StartTokioRuntime,

    #[error("Unexpected servicing state '{state:?}'")]
    UnexpectedServicingState { state: ServicingState },

    #[error("Failed to build storage graph: {0}")]
    RebuildStorageGraph(#[from] StorageGraphBuildError),

    #[error("Failed to wait for 'systemd-networkd'")]
    WaitForSystemdNetworkd,
}

/// Identifies errors that occur when the user provides an invalid input.
#[derive(Debug, Eq, thiserror::Error, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum InvalidInputError {
    #[error("Allowed operations must be passed via command line, not in Host Configuration")]
    AllowedOperationsInHostConfiguration,

    #[error("Failed to initialize clean install as host is already provisioned")]
    CleanInstallOnProvisionedHost,

    #[error("Failed to get a unique Host Configuration source from local Trident config")]
    GetHostConfigurationSource,

    #[error("Host Configuration failed dynamic validation: {inner}")]
    InvalidHostConfigurationDynamic {
        #[from]
        inner: HostConfigurationDynamicValidationError,
    },

    #[error("Host Configuration failed static validation: {inner}")]
    InvalidHostConfigurationStatic {
        #[from]
        inner: HostConfigurationStaticValidationError,
    },

    #[error("Invalid internal parameter '{name}' provided: '{explanation}'")]
    InvalidInternalParameter { name: String, explanation: String },

    #[error("Failed to load COSI file from '{url}'")]
    LoadCosi { url: Url },

    #[error("Failed to load Host Configuration file from '{path}'")]
    LoadHostConfigurationFile { path: String },

    #[error("Failed to load kickstart file from '{path}'")]
    LoadKickstart { path: String },

    #[error("Provided '{actual}' architecture OS image, but system is '{expected}'")]
    MismatchedArchitecture {
        expected: &'static str,
        actual: &'static str,
    },

    #[error("Provided '{hc_fs_type}' filesystem type at '{mount_point}' in Host Configuration, but found '{os_img_fs_type}' filesystem type in the OS image")]
    MismatchedFsType {
        mount_point: String,
        hc_fs_type: String,
        os_img_fs_type: String,
    },

    #[error("An OS image must be provided.")]
    MissingOsImage,

    #[error("Filesystem at '{mount_point}' of type '{fs_type}' in Host Configuration could not be found in the OS image")]
    MissingOsImageFilesystem {
        mount_point: String,
        fs_type: String,
    },

    #[error("Old style configuration not supported, 'hostConfiguration:' tag must be removed")]
    OldStyleConfiguration,

    #[error("Failed to parse Host Configuration file from '{path}'")]
    ParseHostConfigurationFile { path: String },

    #[error("Failed to read input file '{path}'")]
    ReadInputFile { path: String },

    #[error(
        "Root verity configuration in OS Image does not match Host Configuration. Expected OS \
        Image to {}have root verity enabled.", 
        if *hc_verity_status { "" } else { "not " }
    )]
    RootVerityMismatch { hc_verity_status: bool },

    #[error(
        "SELinux is enabled in the Host Configuration, but SELinux could not be found on the image: {0}"
    )]
    SelinuxEnabledButNotFound(String),

    #[error("Failed to translate kickstart")]
    TranslateKickstart,

    #[error("Found verity hash on ESP image. ESP filesystem should never have verity enabled.")]
    UnexpectedVerityOnEsp,

    #[error("Filesystem at '{mount_point}' of type '{fs_type}' in OS Image is not being used by the provided Host Configuration")]
    UnusedOsImageFilesystem {
        mount_point: String,
        fs_type: String,
    },

    #[error("Failed to write to output file '{path}'")]
    WriteOutputFile { path: String },
}

/// Identifies errors that occur during servicing and require further user investigation, to
/// determine whether the error occurred due to an internal failure in Trident, a failure in
/// one of its dependencies, or a system misconfiguration.
#[derive(Debug, Eq, thiserror::Error, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ServicingError {
    #[error("A/B update failed as host booted from '{root_device_path}' instead of the expected device '{expected_device_path}")]
    AbUpdateRebootCheck {
        root_device_path: String,
        expected_device_path: String,
    },

    #[error("Failed to generate Netplan config")]
    GenerateNetplanConfig,

    #[error("Failed to check if the boot entry '{boot_entry}' exists via efibootmgr")]
    BootEntryCheck { boot_entry: String },

    #[error("Failed to canonicalize path '{path}'")]
    CanonicalizePath { path: String },

    #[error("Failed to check if '{path}' is a mount point")]
    CheckIfMountPoint { path: String },

    #[error("Failed to mount special directory '{dir}' for chroot")]
    ChrootMountSpecialDir { dir: String },

    #[error("Failed to unmount special directory for chroot")]
    ChrootUnmountSpecialDir,

    #[error("Clean install failed as host booted from '{root_device_path}' instead of the expected device '{expected_device_path}")]
    CleanInstallRebootCheck {
        root_device_path: String,
        expected_device_path: String,
    },

    #[error("Failed to clean up pre-existing LUKS2-encrypted volumes")]
    CleanupEncryption,

    #[error("Failed to clean up pre-existing RAID arrays")]
    CleanupRaid,

    #[error("Failed to clean up pre-existing verity devices")]
    CleanupVerity,

    #[error("Failed to execute command")]
    CommandCouldNotExecute { binary: &'static str },

    #[error("Command '{binary}' failed: {explanation}")]
    CommandFailed {
        binary: &'static str,
        explanation: String,
    },

    #[error("Failed to stage machine-id file")]
    CopyMachineId,

    #[error("Failed to copy Trident binary to runtime OS")]
    CopyTridentBinary,

    #[error("Failed to create boot entry '{boot_entry}' via efibootmgr")]
    CreateBootEntry { boot_entry: String },

    #[error("Failed to create crypttab at path '{crypttab_path}'")]
    CreateCrypttab { crypttab_path: String },

    #[error("Failed to create directory '{dir}'")]
    CreateDirectory { dir: String },

    #[error("Failed to create execroot directory")]
    CreateExecrootDirectory,

    #[error("Failed to create filesystems")]
    CreateFilesystems,

    #[error("Failed to create machine ID for verity")]
    CreateMachineId,

    #[error("Failed to create mdadm.conf file after RAID creation")]
    CreateMdadmConf,

    #[error("Failed to create disk partitions")]
    CreatePartitions,

    #[error("Failed to create software RAID")]
    CreateRaid,

    #[error("Failed to create temporary recovery key file")]
    CreateRecoveryKeyFile,

    #[error("Failed to create Trident config file")]
    CreateTridentConfig,

    #[error("Failed to create Trident config directory")]
    CreateTridentConfigDirectory,

    #[error("Failed to create verity devices")]
    CreateVerity,

    #[error(transparent)]
    Datastore {
        #[from]
        inner: DatastoreError,
    },

    #[error("Failed to perform file-based deployment of ESP images")]
    DeployESPImages,

    #[error("Failed to deploy images")]
    DeployImages,

    #[error("Failed to encrypt and open block device '{device_path}' with id '{device_id}' as '{encrypted_volume_device_name}' for encrypted volume '{encrypted_volume}'")]
    EncryptBlockDevice {
        device_path: String,
        device_id: String,
        encrypted_volume_device_name: String,
        encrypted_volume: String,
    },

    #[error("Failed to enter chroot")]
    EnterChroot,

    #[error("Failed to exit chroot")]
    ExitChroot,

    #[error("Failed to find underlying block device with id '{device_id}' for encrypted volume '{encrypted_volume}'")]
    FindEncryptedVolumeBlockDevice {
        device_id: String,
        encrypted_volume: String,
    },

    #[error("Failed to find staged file at path '{staged_file}'")]
    FindStagedFile { staged_file: String },

    #[error("Failed to generate fstab at path '{fstab_path}'")]
    GenerateFstab { fstab_path: String },

    #[error("Failed to generate recovery key file '{key_file}'")]
    GenerateRecoveryKeyFile { key_file: String },

    #[error("Failed to get block device path for device '{device_id}'")]
    GetBlockDevicePath { device_id: String },

    #[error("Failed to get the disks to rebuild")]
    GetDisksToRebuild,

    #[error("Failed to get the ESP device information")]
    GetEspDeviceInfo,

    #[error("Failed to get the label and path for the EFI boot loader of the A/B update volume")]
    GetLabelandPath,

    #[error("Failed to get the partition number of '{part_uuid_path}' in the disk '{disk_path}'")]
    GetPartitionNumber {
        disk_path: String,
        part_uuid_path: String,
    },

    #[error("Failed to resolve disks to device paths")]
    GetResolvedDisks,

    #[error("Failed to get mount point info for root with path '{root_path}'")]
    GetRootMountPointInfo { root_path: String },

    #[error("Failed to get block device path of root verity data device")]
    GetRootVerityDataDevPath,

    #[error("Failed to get configuration for root verity device")]
    GetRootVerityDeviceConfig,

    #[error("Failed to get SELINUX")]
    GetSelinuxMode,

    #[error("Failed to get SELINUXTYPE")]
    GetSelinuxType,

    #[error("Failed to perform kexec")]
    Kexec,

    #[error("Failed to list boot entries via efibootmgr or parse them")]
    ListAndParseBootEntries,

    #[error("Failed to mount execroot binary")]
    MountExecrootBinary,

    #[error("Failed to mount newroot")]
    MountNewroot,

    #[error("Failed to mount special directory '{dir}' in newroot")]
    MountNewrootSpecialDir { dir: String },

    #[error("Failed to mount overlay '{target}'")]
    MountOverlay { target: String },

    #[error("Failed to open firewall")]
    OpenFirewall,

    #[error("Failed to parse non-Unicode path '{path}'")]
    PathIsNotUnicode { path: String },

    #[error("Failed to do a read operation with efibootmgr")]
    ReadEfibootmgr,

    #[error("Failed to read current system hostname from {path}")]
    ReadHostname { path: String },

    #[error("Failed to reboot")]
    Reboot,

    #[error("Reboot timed out")]
    RebootTimeout,

    #[error("Failed to rebuild RAID arrays")]
    RebuildRaid,

    #[error("Failed to regenerate initrd")]
    RegenerateInitrd,

    #[error("Failed to remove crypttab at path '{crypttab_path}'")]
    RemoveCrypttab { crypttab_path: String },

    #[error("Failed to run pre-servicing script '{script_name}'")]
    RunPreServicingScript { script_name: String },

    #[error("Failed to run post-configure script '{script_name}'")]
    RunPostConfigureScript { script_name: String },

    #[error("Failed to run OS modifier")]
    RunOsModifier,

    #[error("Failed to run post-provision script '{script_name}'")]
    RunPostProvisionScript { script_name: String },

    #[error("Failed to set permissions on temporary recovery key file '{key_file}'")]
    SetRecoveryKeyFilePermissions { key_file: String },

    #[error("Failed to set up users for management OS")]
    SetUpUsers,

    #[error("Failed to start network")]
    StartNetwork,

    #[error("Trident rebuild-raid validation failed")]
    ValidateRebuildRaid,

    #[error("Failed to unmount newroot, unable to unmount '{dir}'")]
    UnmountNewroot { dir: String },

    #[error("Failed to update `BootOrder` via efibootmgr")]
    UpdateBootOrder,

    #[error("Failed to update GRUB configs")]
    UpdateGrubConfigs,

    #[error("Failed to update GRUB configs after verity creation")]
    UpdateGrubConfigsAfterVerityCreation,

    #[error("Failed to write an additional file '{file_name}'")]
    WriteAdditionalFile { file_name: String },

    #[error("Failed to write Netplan config")]
    WriteNetplanConfig,

    #[error("Failed to validate A/B active volume in Host Status")]
    ValidateAbActiveVolume,
}

/// Identifies errors that occur when interacting with a misconfigured datastore.
#[derive(Debug, Eq, thiserror::Error, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum DatastoreError {
    #[error("Failed to create datastore directory")]
    CreateDatastoreDirectory,

    #[error("Failed to initialize datastore")]
    InitializeDatastore,

    #[error("Failed to load datastore from path '{path}'")]
    LoadDatastore { path: String },

    #[error("Failed to open new datastore")]
    OpenDatastore,

    #[error("Failed to switch datastore path to '{new_path}' as datastore is persistent")]
    SwitchPathOnPersistentDatastore { new_path: String },

    #[error("Failed to write to datastore as it is closed")]
    WriteToClosedDatastore,

    #[error("Failed to write to datastore")]
    WriteToDatastore,
}

/// Identifies errors that occur when clean install or update fail due to the current configuration
/// of the host.
#[derive(Debug, Eq, thiserror::Error, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum UnsupportedConfigurationError {
    #[error("No available install index on ESP")]
    NoAvailableInstallIndex,

    #[error("Disk partition(s) no longer exist on system: {partition_ids:?}")]
    PartitionsRemoved { partition_ids: Vec<String> },

    #[error("Failed to find dependency '{name}'")]
    RuntimeDependencyNotFound { name: String },
}

/// Describes different categories of structured errors that can occur in Trident.
///
/// Each variant of `ErrorKind` corresponds to a different category of error. The categories are
/// intended to be meaningful to the user and assist in routing issues to the appropriate team.
///
#[derive(Debug, Eq, thiserror::Error, IntoStaticStr, PartialEq)]
#[strum(serialize_all = "kebab-case")]
pub enum ErrorKind {
    /// Identifies errors that occur when the execution environment is misconfigured.
    #[error(transparent)]
    ExecutionEnvironmentMisconfiguration(#[from] ExecutionEnvironmentMisconfigurationError),

    /// Identifies errors that occur when Trident fails to initialize.
    #[error(transparent)]
    Initialization(#[from] InitializationError),

    /// Identifies errors that occur due to an internal bug or failure in Trident.
    #[error(transparent)]
    Internal(#[from] InternalError),

    /// Identifies errors that occur when the user provides invalid input.
    #[error(transparent)]
    InvalidInput(#[from] InvalidInputError),

    /// Identifies errors that occur during servicing and require further user investigation, to
    /// determine whether the error occurred due to an internal failure in Trident, a failure in
    /// one of its dependencies, or a system misconfiguration.
    #[error(transparent)]
    Servicing(#[from] ServicingError),

    /// Identifies errors that occur when clean install or update fail due to the current
    /// configuration of the host.
    #[error(transparent)]
    UnsupportedConfiguration(#[from] UnsupportedConfigurationError),
}

#[derive(Debug)]
struct TridentErrorInner {
    kind: ErrorKind,
    location: &'static Location<'static>,
    source: Option<anyhow::Error>,
    context: Vec<(Cow<'static, str>, &'static Location<'static>)>,
}

pub struct TridentError(Box<TridentErrorInner>);
impl TridentError {
    #[track_caller]
    pub fn new(kind: impl Into<ErrorKind>) -> Self {
        TridentError(Box::new(TridentErrorInner {
            kind: kind.into(),
            location: Location::caller(),
            source: None,
            context: Vec::new(),
        }))
    }

    #[track_caller]
    pub fn with_source(kind: impl Into<ErrorKind>, source: anyhow::Error) -> Self {
        TridentError(Box::new(TridentErrorInner {
            kind: kind.into(),
            location: Location::caller(),
            source: Some(source),
            context: Vec::new(),
        }))
    }

    #[track_caller]
    pub fn internal(message: &'static str) -> Self {
        Self::new(InternalError::Internal(message))
    }

    pub fn unstructured(self, context: impl Into<Cow<'static, str>>) -> anyhow::Error {
        match self.0.source {
            Some(source) => source.context(self.0.kind).context(context.into()),
            None => anyhow::Error::from(self.0.kind).context(context.into()),
        }
    }

    /// Returns a reference to the inner ErrorKind.
    pub fn kind(&self) -> &ErrorKind {
        &self.0.kind
    }
}

pub trait ReportError<T, K> {
    /// Convert this error into a structured TridentError.
    fn structured(self, kind: K) -> Result<T, TridentError>;
}

impl<T, K> ReportError<T, K> for Option<T>
where
    K: Into<ErrorKind>,
{
    #[track_caller]
    fn structured(self, kind: K) -> Result<T, TridentError> {
        match self {
            Some(t) => Ok(t),
            None => Err(TridentError(Box::new(TridentErrorInner {
                kind: kind.into(),
                location: Location::caller(),
                source: None,
                context: Vec::new(),
            }))),
        }
    }
}

impl<T, E, K> ReportError<T, K> for Result<T, E>
where
    E: Into<anyhow::Error>,
    K: Into<ErrorKind>,
{
    #[track_caller]
    fn structured(self, kind: K) -> Result<T, TridentError> {
        match self {
            Ok(o) => Ok(o),
            Err(e) => Err(TridentError(Box::new(TridentErrorInner {
                kind: kind.into(),
                location: Location::caller(),
                source: Some(e.into()),
                context: Vec::new(),
            }))),
        }
    }
}

pub trait TridentResultExt<T> {
    /// Attach a context message to the error.
    fn message(self, context: impl Into<Cow<'static, str>>) -> Result<T, TridentError>;

    /// Convert the error into an unstructured error.
    fn unstructured(self, context: impl Into<Cow<'static, str>>) -> Result<T, anyhow::Error>;
}
impl<T> TridentResultExt<T> for Result<T, TridentError> {
    #[track_caller]
    fn message(mut self, context: impl Into<Cow<'static, str>>) -> Result<T, TridentError> {
        if let Err(ref mut e) = self {
            e.0.context.push((context.into(), Location::caller()));
        }
        self
    }

    fn unstructured(self, context: impl Into<Cow<'static, str>>) -> Result<T, anyhow::Error> {
        self.map_err(|e| e.unstructured(context))
    }
}

impl Serialize for TridentError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("trident-error", 5)?;
        state.serialize_field("message", &self.0.kind.to_string())?;
        match self.0.kind {
            ErrorKind::ExecutionEnvironmentMisconfiguration(ref e) => {
                state.serialize_field("error", e)?
            }
            ErrorKind::Initialization(ref e) => state.serialize_field("error", e)?,
            ErrorKind::Internal(ref e) => state.serialize_field("error", e)?,
            ErrorKind::InvalidInput(ref e) => state.serialize_field("error", e)?,
            ErrorKind::Servicing(ref e) => state.serialize_field("error", e)?,
            ErrorKind::UnsupportedConfiguration(ref e) => state.serialize_field("error", e)?,
        }
        state.serialize_field("category", <&str>::from(&self.0.kind))?;
        state.serialize_field(
            "location",
            &format!("{}:{}", self.0.location.file(), self.0.location.line()),
        )?;
        match self.0.source {
            Some(ref e) => state.serialize_field("cause", &Some(format!("{:?}", e)))?,
            None => state.serialize_field("cause", &None::<String>)?,
        }
        state.end()
    }
}

impl Debug for TridentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} at {}:{}",
            self.0.kind,
            self.0.location.file(),
            self.0.location.line()
        )?;

        if !self.0.context.is_empty() {
            writeln!(f, "\n\nContext:")?;
            for (i, (context, location)) in self.0.context.iter().enumerate() {
                for (j, line) in context.split('\n').enumerate() {
                    if j == 0 {
                        write!(f, "{: >5}: ", i)?;
                    } else {
                        f.write_str("\n       ")?;
                    }
                    f.write_str(line)?;
                }
                writeln!(f, " at {}:{}", location.file(), location.line())?;
            }
        }

        if let Some(ref source) = self.0.source {
            writeln!(f, "\n\nCaused by:")?;
            let mut index = 0;
            let mut source: Option<&dyn std::error::Error> = Some(source.as_ref());
            while let Some(e) = source {
                for (i, line) in e.to_string().split('\n').enumerate() {
                    if i == 0 {
                        write!(f, "{: >5}: ", index)?;
                    } else {
                        f.write_str("\n       ")?;
                    }
                    f.write_str(line)?;
                }
                f.write_char('\n')?;
                source = e.source();
                index += 1;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use anyhow::Context;
    use serde_yaml::Value;

    use super::*;

    #[test]
    fn test_error_serialize() {
        let e = TridentError(Box::new(TridentErrorInner {
            kind: ErrorKind::Initialization(InitializationError::LoadHostStatus),
            location: Location::caller(),
            source: Some(
                std::fs::read("/non-existant-file")
                    .context("Failed to read file")
                    .unwrap_err(),
            ),
            context: Vec::new(),
        }));
        match serde_yaml::to_value(e).unwrap() {
            Value::Mapping(m) => {
                assert_eq!(m.len(), 5);
                assert_eq!(m["error"], Value::String("load-host-status".into()));
                assert_eq!(m["category"], Value::String("initialization".into()));
                assert!(matches!(m["cause"], Value::String(_)));
                assert_eq!(
                    m["message"],
                    Value::String("Failed to load local Trident Host Status".into())
                );
                match m["location"] {
                    Value::String(ref s) => assert!(s.contains("error.rs:")),
                    _ => panic!("Location isn't string"),
                }
            }
            _ => panic!("Value isn't mapping"),
        }
    }

    #[test]
    fn test_error_debug() {
        let error = Err::<(), _>(anyhow::anyhow!("z"))
            .context("x\ny")
            .structured(InternalError::Internal("w"))
            .unwrap_err();
        assert_eq!(
            format!("{:?}", error),
            format!(
                "Internal error: w at {}:{}\n\nCaused by:\n    0: x\n       y\n    1: z\n",
                error.0.location.file(),
                error.0.location.line(),
            ),
        );
    }
}
