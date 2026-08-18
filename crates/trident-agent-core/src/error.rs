use serde::{Deserialize, Serialize};

#[derive(Debug, Eq, thiserror::Error, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum AgentCoreError {
    #[error("Failed to initialize the agent client: {0}")]
    InitializationError(String),

    #[error("The version provided '{version}' is not valid semver: {inner}")]
    InvalidVersion { version: String, inner: String },

    #[error("Failed to read machine-id: {0}")]
    MachineIdRead(String),

    #[error("Failed to read hostname: {0}")]
    HostnameRead(String),

    #[error("Internal error: {0}")]
    Internal(String),

    /// Wraps a [`nebraska::NebraskaError`](crate::nebraska::NebraskaError).
    /// Stored as a string rather than `#[from]` because `NebraskaError`
    /// doesn't derive `Serialize`/`Deserialize`/`PartialEq`, which
    /// `AgentCoreError` requires for annotation-status round-tripping.
    #[error("Nebraska request failed: {0}")]
    Nebraska(String),
}
