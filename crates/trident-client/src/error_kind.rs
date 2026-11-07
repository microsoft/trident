use std::time::Duration;

use thiserror::Error;

use crate::{protocol::Operation, ExecutionState, RemoteError, RpcCode};

#[derive(Debug, Error)]
pub(crate) enum ClientErrorKind {
    #[error("invalid Trident endpoint '{endpoint}': {details}")]
    InvalidEndpoint { endpoint: String, details: String },

    #[error("failed to connect to Trident endpoint '{endpoint}': {details}")]
    Connection { endpoint: String, details: String },

    #[error("invalid input for {operation:?}: {details}")]
    InvalidInput {
        operation: Operation,
        details: String,
    },

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
