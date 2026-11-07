use std::{error::Error, fmt};

use trident_api::primitives::hash::Sha384Hash;

/// Selects who performs a reboot required by servicing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum RebootPolicy {
    Trident,
    Caller,
}

/// Final reboot disposition reported by `tridentd`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum RebootDisposition {
    NotRequired,
    Required,
    Started,
}

/// Manual rollback selection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum RollbackKind {
    Any,
    Ab,
    Runtime,
}

/// Timestamp carried by a servicing event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EventTimestamp {
    pub seconds: i64,
    pub nanos: i32,
}

/// Location in Trident source code.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceLocation {
    pub path: String,
    pub line: u32,
}

/// Log severity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum LogLevel {
    Unspecified,
    Error,
    Warn,
    Info,
    Debug,
    Trace,
    Unknown(i32),
}

/// Log record emitted during servicing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LogRecord {
    pub timestamp: Option<EventTimestamp>,
    pub level: LogLevel,
    pub message: String,
    pub module: String,
    pub target: String,
    pub location: Option<SourceLocation>,
}

/// Major structured remote-error category.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum RemoteErrorCategory {
    Unspecified,
    ExecutionEnvironmentMisconfiguration,
    HealthChecks,
    Initialization,
    Internal,
    InvalidInput,
    Servicing,
    UnsupportedConfiguration,
    Unknown(i32),
}

/// Structured error returned by `tridentd`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteError {
    pub category: RemoteErrorCategory,
    pub subkind: String,
    pub message: String,
    pub error_message: String,
    pub location: Option<SourceLocation>,
}

impl RemoteError {
    /// Underlying error message reported by Trident.
    pub fn cause(&self) -> &str {
        &self.error_message
    }
}

impl fmt::Display for RemoteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for RemoteError {}

/// Successful servicing completion.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Completion {
    pub reboot: RebootDisposition,
    pub servicing_kind: Option<ServicingKind>,
    pub image_hash: Option<ImageHash>,
}

/// Kind of servicing reported by a successful completion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ServicingKind {
    Unspecified,
    NoneRequired,
    RuntimeUpdate,
    AbUpdate,
    CleanInstall,
    ManualRollbackAb,
    ManualRollbackRuntime,
    Unknown(i32),
}

/// Image hash reported by a successful completion.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ImageHash {
    Sha384(Sha384Hash),
    Unknown(String),
}

/// Domain event emitted by a servicing operation.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ServicingEvent {
    Started { timestamp: Option<EventTimestamp> },
    Log(LogRecord),
    Completed(Result<Completion, RemoteError>),
}

#[cfg(feature = "preview")]
/// Result of static Host Configuration validation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidationResult {
    pub valid: bool,
    pub error: Option<RemoteError>,
}

#[cfg(feature = "preview")]
/// Rollback availability reported by preview rollback inspection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum RollbackAvailability {
    Unspecified,
    None,
    Ab,
    Runtime,
    Unknown(i32),
}

#[cfg(feature = "preview")]
/// Current servicing state reported by `tridentd`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ServicingState {
    Unspecified,
    NotProvisioned,
    CleanInstallStaged,
    AbUpdateStaged,
    ManualRollbackAbStaged,
    ManualRollbackRuntimeStaged,
    RuntimeUpdateStaged,
    CleanInstallFinalized,
    AbUpdateFinalized,
    ManualRollbackAbFinalized,
    Provisioned,
    AbUpdateHealthCheckFailed,
    Unknown(i32),
}

#[cfg(feature = "preview")]
/// Active A/B volume reported by `tridentd`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ActiveVolume {
    Unspecified,
    None,
    VolumeA,
    VolumeB,
    Unknown(i32),
}
