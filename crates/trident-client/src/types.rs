use trident_api::{primitives::hash::Sha384Hash, status::ServicingType};

/// Identifies a client operation independently of its transport method.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operation {
    Version,
    StreamDisk,
    Update,
    UpdateStage,
    UpdateFinalize,
    Commit,
    Rollback,
    RollbackStage,
    RollbackFinalize,
    #[cfg(feature = "preview")]
    Install,
    #[cfg(feature = "preview")]
    InstallStage,
    #[cfg(feature = "preview")]
    InstallFinalize,
    #[cfg(feature = "preview")]
    RebuildRaid,
    #[cfg(feature = "preview")]
    ProvisionedConfig,
    #[cfg(feature = "preview")]
    ServicingConfig,
    #[cfg(feature = "preview")]
    LastError,
    #[cfg(feature = "preview")]
    ServicingState,
    #[cfg(feature = "preview")]
    ActiveVolume,
    #[cfg(feature = "preview")]
    ValidateHostConfiguration,
    #[cfg(feature = "preview")]
    RequiredServicingType,
    #[cfg(feature = "preview")]
    CheckRollback,
    #[cfg(feature = "preview")]
    RollbackChain,
    #[cfg(feature = "preview")]
    RollbackTarget,
    #[cfg(feature = "preview")]
    CheckRoot,
}

/// Selects who performs a reboot required by servicing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RebootPolicy {
    Trident,
    Caller,
}

/// Final reboot disposition reported by `tridentd`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RebootDisposition {
    NotRequired,
    Required,
    Started,
}

/// Manual rollback selection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
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
pub enum RemoteErrorCategory {
    ExecutionEnvironmentMisconfiguration,
    HealthChecks,
    Initialization,
    Internal,
    InvalidInput,
    Servicing,
    UnsupportedConfiguration,
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

/// Successful servicing completion.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Completion {
    pub reboot: RebootDisposition,
    pub servicing_type: Option<ServicingType>,
    pub image_hash: Option<Sha384Hash>,
}

/// Terminal servicing outcome.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OperationOutcome {
    Succeeded(Completion),
    Failed(RemoteError),
}

/// Domain event emitted by a servicing operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ServicingEvent {
    Started { timestamp: Option<EventTimestamp> },
    Log(LogRecord),
    Completed(OperationOutcome),
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
pub enum RollbackAvailability {
    None,
    Ab,
    Runtime,
}
