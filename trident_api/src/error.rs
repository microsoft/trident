use std::fmt::{Debug, Write};
use std::path::PathBuf;
use std::{borrow::Cow, panic::Location};

use serde::{ser::SerializeStruct, Deserialize, Serialize};
use strum_macros::IntoStaticStr;

use crate::config::InvalidHostConfigurationError;

/// Trident failed to initialize.
#[derive(Debug, Eq, thiserror::Error, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum InitializationError {
    #[error("Failed load local configuration")]
    LoadLocalConfig,
    #[error("Failed to parse local configuration")]
    ParseLocalConfig,
    #[error("Failed connecting to logstream")]
    StartLogstream,
    #[error("Failed to load datastore from '{path}'")]
    DatastoreLoad { path: String },
    #[error("Failed to open datastore")]
    DatastoreOpen,
    #[error("Failed to get host root path")]
    GetHostRootPath,
    #[error("Trident directed to perform clean install but safety check failed")]
    SafetyCheck,
    #[error("Container configuration check failed")]
    ContainerMisconfigured,
}

/// Trident failed to run because the execution environment was misconfigured.
/// This is a user attributable error as it relates to the environment in which
/// Trident is running, which is user defined.
#[derive(Debug, Eq, thiserror::Error, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ExecutionEnvironmentMisconfigurationError {
    #[error(
        "Selected operation cannot be performed due to missing permissions, root privileges required"
    )]
    MissingRequiredPermissions,
}

/// User provided input was invalid.
#[derive(Debug, Eq, thiserror::Error, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum InvalidInputError {
    #[error("Failed to load host configuration file from '{path}'")]
    LoadHostConfiguration { path: String },
    #[error("Failed to parse host configuration")]
    ParseHostConfiguration,
    #[error("Failed to load kickstart file from '{path}'")]
    LoadKickstart { path: String },
    #[error("Failed to translate kickstart")]
    KickstartTranslation,
    #[error("Invalid host configuration: {0}")]
    InvalidHostConfiguration(#[from] InvalidHostConfigurationError),
    #[error("Host configuration is incompatible with current install")]
    IncompatibleHostConfiguration,
}

#[derive(Debug, Eq, thiserror::Error, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum UnsupportedConfigurationError {}

#[derive(Debug, Eq, thiserror::Error, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum DatastoreError {
    #[error("Failed to create datastore directory")]
    CreateDatastoreDirectory,
    #[error("Failed to open new datastore")]
    OpenDatastore,
    #[error("Failed to initialize datastore")]
    DatastoreInit,
    #[error("Failed re-open temporary datastore")]
    DatastoreReopen,
    #[error("Failed to persist datastore")]
    PersistDatastore,
    #[error("Failed to serialize host status")]
    SerializeHostStatus,
    #[error("Failed to write to datastore")]
    DatastoreWrite,
    #[error("Attempted to write to closed datastore")]
    DatastoreClosed,
    #[error("Failed to create datastore ref file")]
    CreateDatastoreRefFile,
    #[error("Failed to record datastore location")]
    RecordDatastoreLocation,
}

#[derive(Debug, Eq, thiserror::Error, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ModuleError {
    #[error("{name} module failed to refresh host status")]
    RefreshHostStatus { name: &'static str },
    #[error("{name} module failed to validate host configuration")]
    ValidateHostConfiguration { name: &'static str },
    #[error("{name} module failed to prepare")]
    Prepare { name: &'static str },
    #[error("{name} module failed to provision")]
    Provision { name: &'static str },
    #[error("{name} module failed to configure")]
    Configure { name: &'static str },
}

#[derive(Debug, Eq, thiserror::Error, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ManagementError {
    #[error("Failed to start network")]
    StartNetwork,
    #[error("Failed to open firewall")]
    OpenFirewall,
    #[error("Failed to mount special directory '{dir}' for chroot")]
    ChrootMountSpecial { dir: &'static str },
    #[error("Failed to enter chroot")]
    ChrootEnter,
    #[error("Failed to exit chroot")]
    ChrootExit,
    #[error("Failed to unmount special directory")]
    ChrootUnmountSpecial,
    #[error("Failed to mount overlay")]
    MountOverlay,
    #[error("Failed to unmount overlay")]
    UnmountOverlay,
    #[error("Failed to update host")]
    UpdateHost,
    #[error("Failed to provision host")]
    ProvisionHost,
    #[error("Failed to set boot next")]
    SetBootNext,
    #[error("Failed to mount newroot")]
    MountNewroot,
    #[error("Failed to unmount newroot, unable to unmount '{dir}'")]
    UnmountNewroot { dir: PathBuf },
    #[error("Failed to assemble kernel cmdline")]
    SetKernelCmdline,
    #[error("Failed to perform kexec")]
    Kexec,
    #[error("Failed to reboot")]
    Reboot,
    #[error("Failed to regenerate initrd")]
    RegenerateInitrd,
    #[error("Failed to copy os modifier binary to host")]
    OSModifierCopy,
    #[error(transparent)]
    Module(#[from] ModuleError),
    #[error(transparent)]
    Datastore(#[from] DatastoreError),
    #[error("Failed to list boot entries")]
    ListBootEntries,
    #[error("Failed to parse boot manager output")]
    ParseEfibootmgrOutput,
    #[error("Failed to modify bootorder")]
    ModifyBootOrder,
    #[error("Failed to clean up pre-existing RAID arrays")]
    CleanupRaid,
    #[error("Failed to create disk partitions")]
    CreatePartitions,
    #[error("Failed to create software RAID")]
    CreateRaid,
    #[error("Failed to create encrypted volumes")]
    CreateEncryptedVolumes,
    #[error("Failed to deploy images")]
    DeployImages,
    #[error("Failed to create filesystems")]
    CreateFilesystems,
}

#[derive(Debug, Eq, thiserror::Error, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum InternalError {
    #[error("Internal error: {0}")]
    Internal(&'static str),
    #[error("An uncategorized error occurred: {0}")]
    Todo(&'static str),

    #[error("Failed to get root block device")]
    GetRootBlockDevice,
    #[error("Failed to serialize host status")]
    SerializeHostStatus,
    #[error("Failed to send host status")]
    SendHostStatus,
}

/// Each variant of `ErrorKind` corresponds to a different category of error. The categories are
/// intended to be user-meaningful and to be used for routing issues to the proper team.
#[derive(Debug, Eq, thiserror::Error, IntoStaticStr, PartialEq)]
#[strum(serialize_all = "kebab-case")]
pub enum ErrorKind {
    /// Trident failed to initialize.
    #[error(transparent)]
    Initialization(#[from] InitializationError),

    /// Trident failed to run because the execution environment was misconfigured.
    #[error(transparent)]
    ExecutionEnvironmentMisconfiguration(#[from] ExecutionEnvironmentMisconfigurationError),

    /// Trident failed because it was provided invalid user input.
    #[error(transparent)]
    InvalidInput(#[from] InvalidInputError),

    /// Trident was unable to provision or update due to the current configuration of the system.
    #[error(transparent)]
    UnsupportedConfiguration(#[from] UnsupportedConfigurationError),

    /// Some step during provisioning or update failed. User investigation is required to determine
    /// whether this is an issue with Trident or one of its dependencies, or whether the system is
    /// misconfigured.
    #[error(transparent)]
    Management(#[from] ManagementError),

    /// An uncategorized error occurred or a bug was encountered. This indicates a problem with
    /// Trident.
    #[error(transparent)]
    Internal(#[from] InternalError),
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
    pub fn secondary_error_context(mut self, secondary: TridentError) -> Self {
        self.0.context.push((format!(
            "While handling the error, an additional error was caught: \n\n{secondary:?}\n\nThe earlier error:"
        ).into(), Location::caller()));
        self
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
            ErrorKind::Initialization(ref e) => state.serialize_field("error", e)?,
            ErrorKind::ExecutionEnvironmentMisconfiguration(ref e) => {
                state.serialize_field("error", e)?
            }
            ErrorKind::InvalidInput(ref e) => state.serialize_field("error", e)?,
            ErrorKind::UnsupportedConfiguration(ref e) => state.serialize_field("error", e)?,
            ErrorKind::Management(ref e) => state.serialize_field("error", e)?,
            ErrorKind::Internal(ref e) => state.serialize_field("error", e)?,
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
            kind: ErrorKind::Initialization(InitializationError::LoadLocalConfig),
            location: Location::caller(),
            source: Some(
                std::fs::read("/non-existant-file")
                    .context("failed to read file")
                    .unwrap_err(),
            ),
            context: Vec::new(),
        }));
        match serde_yaml::to_value(e).unwrap() {
            Value::Mapping(m) => {
                assert_eq!(m.len(), 5);
                assert_eq!(m["error"], Value::String("load-local-config".into()));
                assert_eq!(m["category"], Value::String("initialization".into()));
                assert!(matches!(m["cause"], Value::String(_)));
                assert_eq!(
                    m["message"],
                    Value::String("Failed load local configuration".into())
                );
                match m["location"] {
                    Value::String(ref s) => assert!(s.contains("error.rs:")),
                    _ => panic!("location isn't string"),
                }
            }
            _ => panic!("value isn't mapping"),
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
