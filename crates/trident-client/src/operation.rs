use std::time::Duration;

use tokio::time;
use tonic::Streaming;

use trident_api::{primitives::hash::Sha384Hash, status::ServicingType};
use trident_proto::v1::{
    servicing_response::Response as ResponseBody, Completed as ProtoCompleted,
    FileLocation as ProtoFileLocation, Log as ProtoLog, LogLevel as ProtoLogLevel,
    RebootStatus as ProtoRebootStatus, ServicingKind as ProtoServicingKind,
    ServicingResponse as ProtoServicingResponse, StatusCode as ProtoStatusCode,
    TridentError as ProtoTridentError, TridentErrorKind as ProtoTridentErrorKind,
};

use crate::{
    ClientError, Completion, EventTimestamp, ExecutionState, LogLevel, LogRecord, Operation,
    OperationOutcome, RebootDisposition, RemoteError, RemoteErrorCategory, ServicingEvent,
    SourceLocation,
};

/// Accepted servicing request with a domain-event response stream.
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
            ResponseBody::Log(record) => {
                ServicingEvent::Log(decode_log(self.operation, timestamp, record)?)
            }
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

fn decode_log(
    operation: Operation,
    timestamp: Option<EventTimestamp>,
    record: ProtoLog,
) -> Result<LogRecord, ClientError> {
    let level = match ProtoLogLevel::try_from(record.level) {
        Ok(ProtoLogLevel::Error) => LogLevel::Error,
        Ok(ProtoLogLevel::Warn) => LogLevel::Warn,
        Ok(ProtoLogLevel::Info) => LogLevel::Info,
        Ok(ProtoLogLevel::Debug) => LogLevel::Debug,
        Ok(ProtoLogLevel::Trace) => LogLevel::Trace,
        Ok(ProtoLogLevel::Unspecified) => {
            return Err(invalid_protocol(
                operation,
                "log level is unspecified",
                ExecutionState::OutcomeUnknown,
            ));
        }
        Err(_) => {
            return Err(invalid_protocol(
                operation,
                format!("log level {} is invalid", record.level),
                ExecutionState::OutcomeUnknown,
            ));
        }
    };

    Ok(LogRecord {
        timestamp,
        level,
        message: record.message,
        module: record.module,
        target: record.target,
        location: record.location.map(decode_location),
    })
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

            let servicing_type = completed
                .servicing_kind
                .map(|kind| decode_servicing_type(operation, kind))
                .transpose()?;
            let image_hash = completed
                .image_hash
                .map(|hash| decode_image_hash(operation, &hash))
                .transpose()?;

            Ok(OperationOutcome::Succeeded(Completion {
                reboot,
                servicing_type,
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
            Ok(OperationOutcome::Failed(decode_remote_error(
                operation, error,
            )?))
        }
    }
}

pub(crate) fn decode_remote_error(
    operation: Operation,
    error: ProtoTridentError,
) -> Result<RemoteError, ClientError> {
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
        Ok(ProtoTridentErrorKind::Unspecified) => {
            return Err(invalid_protocol(
                operation,
                "remote error category is unspecified",
                ExecutionState::Completed,
            ));
        }
        Err(_) => {
            return Err(invalid_protocol(
                operation,
                format!("remote error category {} is invalid", error.kind),
                ExecutionState::Completed,
            ));
        }
    };

    Ok(RemoteError {
        category,
        subkind: error.subkind,
        message: error.message,
        error_message: error.error_message,
        location: error.location.map(decode_location),
    })
}

fn decode_servicing_type(operation: Operation, kind: i32) -> Result<ServicingType, ClientError> {
    match ProtoServicingKind::try_from(kind) {
        Ok(ProtoServicingKind::NoneRequired) => Ok(ServicingType::NoActiveServicing),
        Ok(ProtoServicingKind::RuntimeUpdate) => Ok(ServicingType::RuntimeUpdate),
        Ok(ProtoServicingKind::AbUpdate) => Ok(ServicingType::AbUpdate),
        Ok(ProtoServicingKind::CleanInstall) => Ok(ServicingType::CleanInstall),
        Ok(ProtoServicingKind::ManualRollbackAb) => Ok(ServicingType::ManualRollbackAb),
        Ok(ProtoServicingKind::ManualRollbackRuntime) => Ok(ServicingType::ManualRollbackRuntime),
        Ok(ProtoServicingKind::Unspecified) => Err(invalid_protocol(
            operation,
            "servicing kind is unspecified",
            ExecutionState::Completed,
        )),
        Err(_) => Err(invalid_protocol(
            operation,
            format!("servicing kind {kind} is invalid"),
            ExecutionState::Completed,
        )),
    }
}

fn decode_image_hash(operation: Operation, hash: &str) -> Result<Sha384Hash, ClientError> {
    let value = hash.strip_prefix("sha384:").ok_or_else(|| {
        invalid_protocol(
            operation,
            format!("image hash '{hash}' does not use the sha384 scheme"),
            ExecutionState::Completed,
        )
    })?;
    serde_yaml::from_value(serde_yaml::Value::String(value.to_string())).map_err(|error| {
        invalid_protocol(
            operation,
            format!("image hash '{hash}' is malformed: {error}"),
            ExecutionState::Completed,
        )
    })
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
