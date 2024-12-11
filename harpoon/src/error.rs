use serde::{Deserialize, Serialize};

use crate::omaha::event::{EventResult, OmahaEventType};

#[derive(Debug, Eq, thiserror::Error, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum HarpoonError {
    #[error("The version provided '{version}' is not valid semver: {inner}")]
    InvalidVersion { version: String, inner: String },

    #[error("Failed to read machine-id: {0}")]
    MachineIdRead(String),

    #[error("Internal error: {0}")]
    Internal(String),

    #[error("Failed to send request: {0}")]
    SendRequest(String),

    #[error("Received an HTTP error: {0}")]
    HttpError(String),

    #[error("Failed to parse response: {0}")]
    ParseResponse(String),

    #[error("Received an invalid response from the server: {0}")]
    InvalidResponse(String),

    #[error("Failed to query for updates: {0}")]
    QueryError(String),

    #[error("Failed to fetch the updated document: {0}")]
    FetchError(String),

    #[error(
        "Expected a yaml document, but the provided URL does not have a .yaml extension '{0}'"
    )]
    ExpectedYamlDocument(String),

    #[error("Event '{0:?}:{1:?}' was not acknowledged by server.")]
    EventNotAcknowledged(OmahaEventType, EventResult),
}
