use trident_api::{config::HostConfiguration, primitives::hash::Sha384Hash};
use trident_proto::v1::{
    servicing_response::Response as ResponseBody, Completed as ProtoCompleted,
    FileLocation as ProtoFileLocation, HostConfiguration as ProtoHostConfiguration,
    Log as ProtoLog, LogLevel as ProtoLogLevel, ManualRollbackKind, RebootHandling,
    RebootManagement, RebootStatus as ProtoRebootStatus, ServicingKind as ProtoServicingKind,
    ServicingResponse as ProtoServicingResponse, StatusCode as ProtoStatusCode,
    TridentError as ProtoTridentError, TridentErrorKind as ProtoTridentErrorKind,
};

#[cfg(feature = "preview")]
use trident_proto::v1preview::{
    AbVolumeState, CheckRollbackKind, ServicingState as ProtoServicingState,
};

use crate::{
    ClientError, Completion, EventTimestamp, ExecutionState, ImageHash, LogLevel, LogRecord,
    RebootDisposition, RebootPolicy, RemoteError, RemoteErrorCategory, RollbackKind,
    ServicingEvent, ServicingKind, SourceLocation,
};

#[cfg(feature = "preview")]
use crate::{ActiveVolume, RollbackAvailability, ServicingState, ValidationResult};

const SHA384_HEX_LENGTH: usize = 96;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Operation {
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
    ServicingState,
    #[cfg(feature = "preview")]
    ActiveVolume,
    #[cfg(feature = "preview")]
    ValidateHostConfiguration,
    #[cfg(feature = "preview")]
    CheckRollback,
}

pub(crate) fn serialize_config(
    operation: Operation,
    config: &HostConfiguration,
) -> Result<String, ClientError> {
    serde_yaml::to_string(config)
        .map_err(|error| ClientError::serialization(operation, error.to_string()))
}

pub(crate) fn proto_config(config: impl Into<String>) -> ProtoHostConfiguration {
    ProtoHostConfiguration {
        config: config.into(),
    }
}

pub(crate) fn reboot_management(policy: RebootPolicy) -> RebootManagement {
    let handling = match policy {
        RebootPolicy::Trident => RebootHandling::TridentHandlesReboot,
        RebootPolicy::Caller => RebootHandling::CallerHandlesReboot,
    };
    RebootManagement {
        handling: handling.into(),
    }
}

pub(crate) fn rollback_kind(kind: RollbackKind) -> i32 {
    match kind {
        RollbackKind::Any => ManualRollbackKind::AnyRollbackRequested,
        RollbackKind::Ab => ManualRollbackKind::AbRollbackRequested,
        RollbackKind::Runtime => ManualRollbackKind::RuntimeRollbackRequested,
    }
    .into()
}

pub(crate) fn validate_sha384_hash(hash: &Sha384Hash) -> Result<(), ClientError> {
    let value = hash.as_str();
    if value.len() != SHA384_HEX_LENGTH {
        return Err(ClientError::invalid_input(
            Operation::StreamDisk,
            format!(
                "SHA-384 hash has length {}; expected {SHA384_HEX_LENGTH} hexadecimal characters",
                value.len()
            ),
        ));
    }
    if !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ClientError::invalid_input(
            Operation::StreamDisk,
            "SHA-384 hash contains non-hexadecimal characters",
        ));
    }
    Ok(())
}

pub(crate) fn decode_servicing_response(
    operation: Operation,
    response: ProtoServicingResponse,
) -> Result<ServicingEvent, ClientError> {
    let timestamp = response.timestamp.map(|timestamp| EventTimestamp {
        seconds: timestamp.seconds,
        nanos: timestamp.nanos,
    });
    let body = response.response.ok_or_else(|| {
        ClientError::invalid_protocol(
            operation,
            "servicing response is missing its body",
            ExecutionState::OutcomeUnknown,
        )
    })?;

    match body {
        ResponseBody::Started(_) => Ok(ServicingEvent::Started { timestamp }),
        ResponseBody::Log(record) => Ok(ServicingEvent::Log(decode_log(timestamp, record))),
        ResponseBody::Completed(completed) => Ok(ServicingEvent::Completed(decode_completion(
            operation, completed,
        )?)),
    }
}

pub(crate) fn servicing_response_completes(response: &ProtoServicingResponse) -> bool {
    matches!(response.response.as_ref(), Some(ResponseBody::Completed(_)))
}

fn decode_log(timestamp: Option<EventTimestamp>, record: ProtoLog) -> LogRecord {
    let level = match ProtoLogLevel::try_from(record.level) {
        Ok(ProtoLogLevel::Error) => LogLevel::Error,
        Ok(ProtoLogLevel::Warn) => LogLevel::Warn,
        Ok(ProtoLogLevel::Info) => LogLevel::Info,
        Ok(ProtoLogLevel::Debug) => LogLevel::Debug,
        Ok(ProtoLogLevel::Trace) => LogLevel::Trace,
        Ok(ProtoLogLevel::Unspecified) => LogLevel::Unspecified,
        Err(_) => LogLevel::Unknown(record.level),
    };

    LogRecord {
        timestamp,
        level,
        message: record.message,
        module: record.module,
        target: record.target,
        location: record.location.map(decode_location),
    }
}

fn decode_completion(
    operation: Operation,
    completed: ProtoCompleted,
) -> Result<Result<Completion, RemoteError>, ClientError> {
    let status = ProtoStatusCode::try_from(completed.status).map_err(|_| {
        ClientError::invalid_protocol(
            operation,
            format!("completion status {} is invalid", completed.status),
            ExecutionState::Completed,
        )
    })?;

    match status {
        ProtoStatusCode::Unspecified => Err(ClientError::invalid_protocol(
            operation,
            "completion status is unspecified",
            ExecutionState::Completed,
        )),
        ProtoStatusCode::Success => {
            if completed.error.is_some() {
                return Err(ClientError::invalid_protocol(
                    operation,
                    "successful completion carries an error",
                    ExecutionState::Completed,
                ));
            }

            let reboot = match ProtoRebootStatus::try_from(completed.reboot_status) {
                Ok(ProtoRebootStatus::RebootNotRequired) => RebootDisposition::NotRequired,
                Ok(ProtoRebootStatus::RebootRequired) => RebootDisposition::Required,
                Ok(ProtoRebootStatus::RebootStarted) => RebootDisposition::Started,
                Ok(ProtoRebootStatus::Unspecified) => {
                    return Err(ClientError::invalid_protocol(
                        operation,
                        "successful completion has unspecified reboot status",
                        ExecutionState::Completed,
                    ));
                }
                Err(_) => {
                    return Err(ClientError::invalid_protocol(
                        operation,
                        format!(
                            "successful completion has invalid reboot status {}",
                            completed.reboot_status
                        ),
                        ExecutionState::Completed,
                    ));
                }
            };

            let servicing_kind = completed.servicing_kind.map(decode_servicing_kind);
            let image_hash = completed.image_hash.map(decode_image_hash);

            Ok(Ok(Completion {
                reboot,
                servicing_kind,
                image_hash,
            }))
        }
        ProtoStatusCode::Failure => {
            let error = completed.error.ok_or_else(|| {
                ClientError::invalid_protocol(
                    operation,
                    "failed completion is missing structured error details",
                    ExecutionState::Completed,
                )
            })?;
            Ok(Err(decode_remote_error(error)))
        }
    }
}

pub(crate) fn decode_remote_error(error: ProtoTridentError) -> RemoteError {
    let category = match ProtoTridentErrorKind::try_from(error.kind) {
        Ok(ProtoTridentErrorKind::ExecutionEnvironmentMisconfigurationError) => {
            RemoteErrorCategory::ExecutionEnvironmentMisconfiguration
        }
        Ok(ProtoTridentErrorKind::HealthChecksError) => RemoteErrorCategory::HealthChecks,
        Ok(ProtoTridentErrorKind::InitializationError) => RemoteErrorCategory::Initialization,
        Ok(ProtoTridentErrorKind::InternalError) => RemoteErrorCategory::Internal,
        Ok(ProtoTridentErrorKind::InvalidInputError) => RemoteErrorCategory::InvalidInput,
        Ok(ProtoTridentErrorKind::ServicingError) => RemoteErrorCategory::Servicing,
        Ok(ProtoTridentErrorKind::UnsupportedConfigurationError) => {
            RemoteErrorCategory::UnsupportedConfiguration
        }
        Ok(ProtoTridentErrorKind::Unspecified) => RemoteErrorCategory::Unspecified,
        Err(_) => RemoteErrorCategory::Unknown(error.kind),
    };

    RemoteError {
        category,
        subkind: error.subkind,
        message: error.message,
        error_message: error.error_message,
        location: error.location.map(decode_location),
    }
}

fn decode_servicing_kind(kind: i32) -> ServicingKind {
    match ProtoServicingKind::try_from(kind) {
        Ok(ProtoServicingKind::Unspecified) => ServicingKind::Unspecified,
        Ok(ProtoServicingKind::NoneRequired) => ServicingKind::NoneRequired,
        Ok(ProtoServicingKind::RuntimeUpdate) => ServicingKind::RuntimeUpdate,
        Ok(ProtoServicingKind::AbUpdate) => ServicingKind::AbUpdate,
        Ok(ProtoServicingKind::CleanInstall) => ServicingKind::CleanInstall,
        Ok(ProtoServicingKind::ManualRollbackAb) => ServicingKind::ManualRollbackAb,
        Ok(ProtoServicingKind::ManualRollbackRuntime) => ServicingKind::ManualRollbackRuntime,
        Err(_) => ServicingKind::Unknown(kind),
    }
}

fn decode_image_hash(hash: String) -> ImageHash {
    match hash.strip_prefix("sha384:") {
        Some(value)
            if value.len() == SHA384_HEX_LENGTH
                && value.bytes().all(|byte| byte.is_ascii_hexdigit()) =>
        {
            ImageHash::Sha384(Sha384Hash::from(value))
        }
        _ => ImageHash::Unknown(hash),
    }
}

fn decode_location(location: ProtoFileLocation) -> SourceLocation {
    SourceLocation {
        path: location.path,
        line: location.line,
    }
}

#[cfg(feature = "preview")]
pub(crate) fn decode_servicing_state(state: i32) -> ServicingState {
    match ProtoServicingState::try_from(state) {
        Ok(ProtoServicingState::Unspecified) => ServicingState::Unspecified,
        Ok(ProtoServicingState::NotProvisioned) => ServicingState::NotProvisioned,
        Ok(ProtoServicingState::InstallStaged) => ServicingState::CleanInstallStaged,
        Ok(ProtoServicingState::UpdateAbStaged) => ServicingState::AbUpdateStaged,
        Ok(ProtoServicingState::ManualRollbackAbStaged) => ServicingState::ManualRollbackAbStaged,
        Ok(ProtoServicingState::ManualRollbackRuntimeStaged) => {
            ServicingState::ManualRollbackRuntimeStaged
        }
        Ok(ProtoServicingState::UpdateRuntimeStaged) => ServicingState::RuntimeUpdateStaged,
        Ok(ProtoServicingState::InstallFinalized) => ServicingState::CleanInstallFinalized,
        Ok(ProtoServicingState::UpdateAbFinalized) => ServicingState::AbUpdateFinalized,
        Ok(ProtoServicingState::ManualRollbackAbFinalized) => {
            ServicingState::ManualRollbackAbFinalized
        }
        Ok(ProtoServicingState::Provisioned) => ServicingState::Provisioned,
        Ok(ProtoServicingState::UpdateAbHealthCheckFailed) => {
            ServicingState::AbUpdateHealthCheckFailed
        }
        Err(_) => ServicingState::Unknown(state),
    }
}

#[cfg(feature = "preview")]
pub(crate) fn decode_active_volume(active_volume: i32) -> ActiveVolume {
    match AbVolumeState::try_from(active_volume) {
        Ok(AbVolumeState::Unspecified) => ActiveVolume::Unspecified,
        Ok(AbVolumeState::NoVolume) => ActiveVolume::None,
        Ok(AbVolumeState::VolumeA) => ActiveVolume::VolumeA,
        Ok(AbVolumeState::VolumeB) => ActiveVolume::VolumeB,
        Err(_) => ActiveVolume::Unknown(active_volume),
    }
}

#[cfg(feature = "preview")]
pub(crate) fn decode_validation_result(
    ok: bool,
    error: Option<ProtoTridentError>,
) -> Result<ValidationResult, ClientError> {
    let error = error.map(decode_remote_error);
    if ok == error.is_some() {
        return Err(ClientError::invalid_protocol(
            Operation::ValidateHostConfiguration,
            if ok {
                "successful validation carries an error"
            } else {
                "failed validation is missing error details"
            },
            ExecutionState::Completed,
        ));
    }
    Ok(ValidationResult { valid: ok, error })
}

#[cfg(feature = "preview")]
pub(crate) fn decode_rollback_availability(kind: i32) -> RollbackAvailability {
    match CheckRollbackKind::try_from(kind) {
        Ok(CheckRollbackKind::Unspecified) => RollbackAvailability::Unspecified,
        Ok(CheckRollbackKind::NoRollbackAvailable) => RollbackAvailability::None,
        Ok(CheckRollbackKind::AbRollbackExpected) => RollbackAvailability::Ab,
        Ok(CheckRollbackKind::RuntimeRollbackExpected) => RollbackAvailability::Runtime,
        Err(_) => RollbackAvailability::Unknown(kind),
    }
}
