use std::{borrow::Cow, panic::Location};

use serde::{ser::SerializeStruct, Deserialize, Serialize};
use strum_macros::IntoStaticStr;

/// Trident failed to initialize.
#[derive(Debug, thiserror::Error, Serialize, Deserialize)]
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
    #[error("Safety check failed")]
    SafetyCheck,
}

/// User provided input was invalid.
#[derive(Debug, thiserror::Error, Serialize, Deserialize)]
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
    #[error("Invalid host configuration")]
    InvalidHostConfiguration,
}

#[derive(Debug, thiserror::Error, Serialize, Deserialize)]
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
}

#[derive(Debug, thiserror::Error, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InternalError {
    #[error("Internal error: {0}")]
    Internal(&'static str),
    #[error("An uncategorized error occurred: {0}")]
    Todo(&'static str),
}

#[derive(Debug, thiserror::Error, IntoStaticStr)]
#[strum(serialize_all = "kebab-case")]
pub enum ErrorKind {
    #[error(transparent)]
    Initialization(#[from] InitializationError),

    #[error(transparent)]
    InvalidInput(#[from] InvalidInputError),

    #[error(transparent)]
    Management(#[from] ManagementError),

    #[error(transparent)]
    InternalError(#[from] InternalError),
}

#[derive(Debug, thiserror::Error)]
#[error("An error occurred")]
struct TridentErrorInner {
    kind: ErrorKind,
    location: &'static Location<'static>,
    source: Option<anyhow::Error>,
    context: Vec<Cow<'static, str>>,
}

#[derive(Debug)]
pub struct TridentError(Box<TridentErrorInner>);
impl TridentError {
    pub fn secondary_error_context(mut self, secondary: TridentError) -> Self {
        self.0.context.push(format!(
            "While handling the error, an additional error was caught: \n\n{:?}\n\nThe earlier error:",
            anyhow::Error::from(secondary.0)
        ).into());
        self
    }
    pub fn unstructured(self, context: impl Into<Cow<'static, str>>) -> anyhow::Error {
        anyhow::Error::from(self.0).context(context.into())
    }
}

impl<T: Into<ErrorKind>> From<T> for TridentError {
    fn from(kind: T) -> Self {
        TridentError(Box::new(TridentErrorInner {
            kind: kind.into(),
            location: Location::caller(),
            source: None,
            context: Vec::new(),
        }))
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
    fn message(mut self, context: impl Into<Cow<'static, str>>) -> Result<T, TridentError> {
        if let Err(ref mut e) = self {
            e.0.context.push(context.into());
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
            ErrorKind::InvalidInput(ref e) => state.serialize_field("error", e)?,
            ErrorKind::Management(ref e) => state.serialize_field("error", e)?,
            ErrorKind::InternalError(ref e) => state.serialize_field("error", e)?,
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
}
