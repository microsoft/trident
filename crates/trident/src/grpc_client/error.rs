use thiserror::Error as ThisError;
use tonic::{transport::Error, Status};

#[derive(ThisError, Debug)]
pub enum TridentClientError {
    #[error("Failed to connect to Trident server at {0}: {1}")]
    ConnectionError(String, #[source] Error),

    #[error("gRPC request '{0}' failed: {1}")]
    RequestError(String, #[source] Status),

    #[error("gRPC response error for '{0}': {1}")]
    ResponseError(String, #[source] Status),

    #[error("Invalid response: {0}")]
    InvalidResponse(String),

    #[error("Servicing error: {0}")]
    ServicingError(String),
}
