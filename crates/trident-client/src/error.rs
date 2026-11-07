use std::{error::Error, fmt, time::Duration};

use tonic::{Code, Status};

use crate::{error_kind::ClientErrorKind, protocol::Operation, RemoteError};

/// Whether a servicing request ran.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ExecutionState {
    /// The request was not accepted by `tridentd`.
    NotStarted,
    /// The request was accepted, but its final outcome is not known.
    OutcomeUnknown,
    /// `tridentd` reported a final outcome.
    Completed,
}

/// Transport-independent gRPC status code.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum RpcCode {
    Ok,
    Cancelled,
    Unknown,
    InvalidArgument,
    DeadlineExceeded,
    NotFound,
    AlreadyExists,
    PermissionDenied,
    ResourceExhausted,
    FailedPrecondition,
    Aborted,
    OutOfRange,
    Unimplemented,
    Internal,
    Unavailable,
    DataLoss,
    Unauthenticated,
}

impl From<Code> for RpcCode {
    fn from(code: Code) -> Self {
        match code {
            Code::Ok => Self::Ok,
            Code::Cancelled => Self::Cancelled,
            Code::Unknown => Self::Unknown,
            Code::InvalidArgument => Self::InvalidArgument,
            Code::DeadlineExceeded => Self::DeadlineExceeded,
            Code::NotFound => Self::NotFound,
            Code::AlreadyExists => Self::AlreadyExists,
            Code::PermissionDenied => Self::PermissionDenied,
            Code::ResourceExhausted => Self::ResourceExhausted,
            Code::FailedPrecondition => Self::FailedPrecondition,
            Code::Aborted => Self::Aborted,
            Code::OutOfRange => Self::OutOfRange,
            Code::Unimplemented => Self::Unimplemented,
            Code::Internal => Self::Internal,
            Code::Unavailable => Self::Unavailable,
            Code::DataLoss => Self::DataLoss,
            Code::Unauthenticated => Self::Unauthenticated,
        }
    }
}

/// Failure returned by the client.
///
/// The concrete failure representation is intentionally private. Use
/// [`execution_state`](Self::execution_state),
/// [`remote_error`](Self::remote_error), and [`rpc_code`](Self::rpc_code) for
/// actionable error metadata.
pub struct ClientError {
    kind: ClientErrorKind,
}

impl fmt::Debug for ClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.kind, formatter)
    }
}

impl Error for ClientError {}

impl ClientError {
    pub(crate) fn invalid_endpoint(endpoint: String, details: String) -> Self {
        Self {
            kind: ClientErrorKind::InvalidEndpoint { endpoint, details },
        }
    }

    pub(crate) fn connection(endpoint: String, details: String) -> Self {
        Self {
            kind: ClientErrorKind::Connection { endpoint, details },
        }
    }

    pub(crate) fn invalid_input(operation: Operation, details: impl Into<String>) -> Self {
        Self {
            kind: ClientErrorKind::InvalidInput {
                operation,
                details: details.into(),
            },
        }
    }

    pub(crate) fn request_rejected(operation: Operation, status: Status) -> Self {
        Self {
            kind: ClientErrorKind::RequestRejected {
                operation,
                code: status.code().into(),
                message: status.message().to_string(),
            },
        }
    }

    pub(crate) fn request_acknowledgement_timeout(operation: Operation, timeout: Duration) -> Self {
        Self {
            kind: ClientErrorKind::RequestAcknowledgementTimeout { operation, timeout },
        }
    }

    pub(crate) fn unary_timeout(operation: Operation, timeout: Duration) -> Self {
        Self {
            kind: ClientErrorKind::UnaryTimeout { operation, timeout },
        }
    }

    pub(crate) fn stream_failure(operation: Operation, status: Status) -> Self {
        Self {
            kind: ClientErrorKind::StreamFailure {
                operation,
                code: status.code().into(),
                message: status.message().to_string(),
            },
        }
    }

    pub(crate) fn missing_completion(operation: Operation) -> Self {
        Self {
            kind: ClientErrorKind::MissingCompletion { operation },
        }
    }

    pub(crate) fn invalid_protocol(
        operation: Operation,
        details: impl Into<String>,
        state: ExecutionState,
    ) -> Self {
        Self {
            kind: ClientErrorKind::InvalidProtocol {
                operation,
                details: details.into(),
                state,
            },
        }
    }

    pub(crate) fn serialization(operation: Operation, details: impl Into<String>) -> Self {
        Self {
            kind: ClientErrorKind::Serialization {
                operation,
                details: details.into(),
            },
        }
    }

    pub(crate) fn completion_timeout(operation: Operation, timeout: Duration) -> Self {
        Self {
            kind: ClientErrorKind::CompletionTimeout { operation, timeout },
        }
    }

    pub(crate) fn remote_failure(operation: Operation, remote: RemoteError) -> Self {
        Self {
            kind: ClientErrorKind::RemoteFailure { operation, remote },
        }
    }

    /// Classifies how certain the client is about remote execution.
    pub fn execution_state(&self) -> ExecutionState {
        match &self.kind {
            ClientErrorKind::InvalidEndpoint { .. }
            | ClientErrorKind::Connection { .. }
            | ClientErrorKind::InvalidInput { .. }
            | ClientErrorKind::Serialization { .. } => ExecutionState::NotStarted,
            ClientErrorKind::RequestRejected {
                code:
                    RpcCode::InvalidArgument | RpcCode::FailedPrecondition | RpcCode::Unimplemented,
                ..
            } => ExecutionState::NotStarted,
            ClientErrorKind::RequestRejected { .. }
            | ClientErrorKind::RequestAcknowledgementTimeout { .. }
            | ClientErrorKind::UnaryTimeout { .. }
            | ClientErrorKind::StreamFailure { .. }
            | ClientErrorKind::MissingCompletion { .. }
            | ClientErrorKind::CompletionTimeout { .. } => ExecutionState::OutcomeUnknown,
            ClientErrorKind::InvalidProtocol { state, .. } => *state,
            ClientErrorKind::RemoteFailure { .. } => ExecutionState::Completed,
        }
    }

    /// Returns structured remote failure details when available.
    pub fn remote_error(&self) -> Option<&RemoteError> {
        match &self.kind {
            ClientErrorKind::RemoteFailure { remote, .. } => Some(remote),
            _ => None,
        }
    }

    /// Returns the gRPC status code when the failure came from an RPC status.
    pub fn rpc_code(&self) -> Option<RpcCode> {
        match &self.kind {
            ClientErrorKind::RequestRejected { code, .. }
            | ClientErrorKind::StreamFailure { code, .. } => Some(*code),
            _ => None,
        }
    }
}
