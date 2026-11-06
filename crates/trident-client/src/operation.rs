use std::time::Duration;

use tokio::time;
use tonic::Streaming;

use trident_api::primitives::hash::Sha384Hash;
use trident_proto::v1::{
    servicing_response::Response as ResponseBody, Completed as ProtoCompleted,
    FileLocation as ProtoFileLocation, Log as ProtoLog, LogLevel as ProtoLogLevel,
    RebootStatus as ProtoRebootStatus, ServicingKind as ProtoServicingKind,
    ServicingResponse as ProtoServicingResponse, StatusCode as ProtoStatusCode,
    TridentError as ProtoTridentError, TridentErrorKind as ProtoTridentErrorKind,
};

use crate::{
    ClientError, Completion, EventTimestamp, ExecutionState, ImageHash, LogLevel, LogRecord,
    Operation, OperationOutcome, RebootDisposition, RemoteError, RemoteErrorCategory,
    ServicingEvent, ServicingKind, SourceLocation,
};

/// Accepted servicing request with a domain-event response stream.
#[must_use = "receiving a ServicingOperation only acknowledges the RPC; call wait, wait_with, or next_event to observe its outcome"]
pub struct ServicingOperation {
    operation: Operation,
    stream: Streaming<ProtoServicingResponse>,
    completed: bool,
    completion_timeout: Option<Duration>,
}

impl ServicingOperation {
    pub(crate) fn new(operation: Operation, stream: Streaming<ProtoServicingResponse>) -> Self {
        Self {
            operation,
            stream,
            completed: false,
            completion_timeout: None,
        }
    }

    /// Applies a timeout to [`wait`](Self::wait) and
    /// [`wait_with`](Self::wait_with). Timing starts when either method is
    /// called, after the request has been accepted.
    pub fn with_completion_timeout(mut self, timeout: Duration) -> Self {
        self.completion_timeout = Some(timeout);
        self
    }

    /// Identifies the RPC that created this operation.
    pub fn operation(&self) -> Operation {
        self.operation
    }

    /// Returns the next domain event.
    pub async fn next_event(&mut self) -> Result<Option<ServicingEvent>, ClientError> {
        if self.completed {
            return Ok(None);
        }

        let response = self
            .stream
            .message()
            .await
            .map_err(|status| ClientError::StreamFailure {
                operation: self.operation,
                code: status.code().into(),
                message: status.message().to_string(),
            })?
            .ok_or(ClientError::MissingCompletion {
                operation: self.operation,
            })?;

        let timestamp = response.timestamp.map(|timestamp| EventTimestamp {
            seconds: timestamp.seconds,
            nanos: timestamp.nanos,
        });
        let body = response.response.ok_or(ClientError::InvalidProtocol {
            operation: self.operation,
            details: "servicing response is missing its body".to_string(),
            state: ExecutionState::OutcomeUnknown,
        })?;

        let event = match body {
            ResponseBody::Started(_) => ServicingEvent::Started { timestamp },
            ResponseBody::Log(record) => ServicingEvent::Log(decode_log(timestamp, record)),
            ResponseBody::Completed(completed) => {
                self.completed = true;
                ServicingEvent::Completed(decode_completion(self.operation, completed)?)
            }
        };
        Ok(Some(event))
    }

    /// Consumes events until successful completion.
    pub async fn wait(self) -> Result<Completion, ClientError> {
        self.wait_with(|_| {}).await
    }

    /// Consumes events, invoking `on_event` for each one.
    pub async fn wait_with<F>(mut self, mut on_event: F) -> Result<Completion, ClientError>
    where
        F: FnMut(&ServicingEvent),
    {
        let operation = self.operation;
        let timeout = self.completion_timeout;
        let wait = async {
            loop {
                let event = self.next_event().await?;
                let Some(event) = event else {
                    return Err(ClientError::MissingCompletion { operation });
                };
                on_event(&event);
                match event {
                    ServicingEvent::Completed(OperationOutcome::Succeeded(completion)) => {
                        return Ok(completion);
                    }
                    ServicingEvent::Completed(OperationOutcome::Failed(remote)) => {
                        return Err(ClientError::RemoteFailure { operation, remote });
                    }
                    ServicingEvent::Started { .. } | ServicingEvent::Log(_) => {}
                }
            }
        };

        match timeout {
            Some(timeout) => time::timeout(timeout, wait)
                .await
                .map_err(|_| ClientError::CompletionTimeout { operation, timeout })?,
            None => wait.await,
        }
    }
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
) -> Result<OperationOutcome, ClientError> {
    let status = ProtoStatusCode::try_from(completed.status).map_err(|_| {
        invalid_protocol(
            operation,
            format!("completion status {} is invalid", completed.status),
            ExecutionState::Completed,
        )
    })?;

    match status {
        ProtoStatusCode::Unspecified => Err(invalid_protocol(
            operation,
            "completion status is unspecified",
            ExecutionState::Completed,
        )),
        ProtoStatusCode::Success => {
            if completed.error.is_some() {
                return Err(invalid_protocol(
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
                    return Err(invalid_protocol(
                        operation,
                        "successful completion has unspecified reboot status",
                        ExecutionState::Completed,
                    ));
                }
                Err(_) => {
                    return Err(invalid_protocol(
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

            Ok(OperationOutcome::Succeeded(Completion {
                reboot,
                servicing_kind,
                image_hash,
            }))
        }
        ProtoStatusCode::Failure => {
            let error = completed.error.ok_or_else(|| {
                invalid_protocol(
                    operation,
                    "failed completion is missing structured error details",
                    ExecutionState::Completed,
                )
            })?;
            Ok(OperationOutcome::Failed(decode_remote_error(error)))
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
        Some(value) if value.len() == 96 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) => {
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

pub(crate) fn invalid_protocol(
    operation: Operation,
    details: impl Into<String>,
    state: ExecutionState,
) -> ClientError {
    ClientError::InvalidProtocol {
        operation,
        details: details.into(),
        state,
    }
}
