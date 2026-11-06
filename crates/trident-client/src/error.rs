use std::time::Duration;

use thiserror::Error;
use tonic::{Code, Status};

use crate::{Operation, RemoteError};

/// Whether a servicing request ran.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
#[derive(Debug, Error)]
pub enum ClientError {
    #[error("invalid Trident endpoint '{endpoint}': {details}")]
    InvalidEndpoint { endpoint: String, details: String },

    #[error("failed to connect to Trident endpoint '{endpoint}': {details}")]
    Connection { endpoint: String, details: String },

    #[error("Trident rejected {operation:?}: {code:?}: {message}")]
    RequestRejected {
        operation: Operation,
        code: RpcCode,
        message: String,
    },

    #[error(
        "Trident request {operation:?} was not acknowledged within {timeout:?}; execution outcome is unknown"
    )]
    RequestAcknowledgementTimeout {
        operation: Operation,
        timeout: Duration,
    },

    #[error("Trident unary request {operation:?} did not complete within {timeout:?}")]
    UnaryTimeout {
        operation: Operation,
        timeout: Duration,
    },

    #[error("Trident stream for {operation:?} failed: {code:?}: {message}")]
    StreamFailure {
        operation: Operation,
        code: RpcCode,
        message: String,
    },

    #[error("Trident stream for {operation:?} ended without completion")]
    MissingCompletion { operation: Operation },

    #[error("invalid Trident protocol response for {operation:?}: {details}")]
    InvalidProtocol {
        operation: Operation,
        details: String,
        state: ExecutionState,
    },

    #[error("failed to serialize input for {operation:?}: {details}")]
    Serialization {
        operation: Operation,
        details: String,
    },

    #[error("failed to deserialize response for {operation:?}: {details}")]
    Deserialization {
        operation: Operation,
        details: String,
    },

    #[error("Trident {operation:?} did not complete within {timeout:?}")]
    CompletionTimeout {
        operation: Operation,
        timeout: Duration,
    },

    #[error("Trident reported {operation:?} failure: {}", remote.message)]
    RemoteFailure {
        operation: Operation,
        remote: RemoteError,
    },
}

impl ClientError {
    /// Classifies how certain the client is about remote execution.
    pub fn execution_state(&self) -> ExecutionState {
        match self {
            Self::InvalidEndpoint { .. } | Self::Connection { .. } | Self::Serialization { .. } => {
                ExecutionState::NotStarted
            }
            Self::RequestRejected {
                code:
                    RpcCode::InvalidArgument | RpcCode::FailedPrecondition | RpcCode::Unimplemented,
                ..
            } => ExecutionState::NotStarted,
            Self::RequestRejected { .. }
            | Self::RequestAcknowledgementTimeout { .. }
            | Self::UnaryTimeout { .. }
            | Self::StreamFailure { .. }
            | Self::MissingCompletion { .. }
            | Self::CompletionTimeout { .. } => ExecutionState::OutcomeUnknown,
            Self::InvalidProtocol { state, .. } => *state,
            Self::Deserialization { .. } | Self::RemoteFailure { .. } => ExecutionState::Completed,
        }
    }

    /// Returns structured remote failure details when available.
    pub fn remote_error(&self) -> Option<&RemoteError> {
        match self {
            Self::RemoteFailure { remote, .. } => Some(remote),
            _ => None,
        }
    }
}

pub(crate) fn rejected(operation: Operation, status: Status) -> ClientError {
    ClientError::RequestRejected {
        operation,
        code: status.code().into(),
        message: status.message().to_string(),
    }
}
