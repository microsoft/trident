//! Error type for the [`nebraska`](crate::nebraska) client module.

use thiserror::Error;

/// Errors that can occur while talking to a Nebraska server.
///
/// Note that several important Nebraska behaviours are deliberately *not*
/// errors, because the protocol treats them as normal outcomes:
///
/// - An update already being in progress for this instance
///   (`error-updateInProgressOnInstance`) is surfaced as
///   [`CheckOutcome::UpdateInProgress`](crate::nebraska::CheckOutcome::UpdateInProgress),
///   not an error. See the protocol spec, §4.
/// - An unrecognised status string never fails parsing; it is preserved in an
///   `Other` variant. See the protocol spec, §7 trap 1.
#[derive(Debug, Error)]
pub enum NebraskaError {
    /// The provided value could not be used to build a valid Omaha request.
    #[error("invalid Nebraska request: {0}")]
    InvalidRequest(String),

    /// Serializing the outgoing request to XML failed.
    #[error("failed to serialize request: {0}")]
    Serialize(String),

    /// The HTTP request to the Nebraska server failed at the transport layer.
    #[error("failed to send request to Nebraska: {0}")]
    Transport(String),

    /// The Nebraska server returned a non-success HTTP status.
    #[error("Nebraska returned an HTTP error: {0}")]
    Http(String),

    /// The response body could not be parsed as an Omaha response.
    #[error("failed to parse Nebraska response: {0}")]
    Parse(String),

    /// The response was well-formed but did not match protocol expectations
    /// (e.g. a missing app, or a mismatched app id).
    #[error("unexpected Nebraska response: {0}")]
    UnexpectedResponse(String),

    /// The Nebraska server reported an error status for the app or update
    /// check. Carries the raw status string for diagnosis.
    #[error("Nebraska reported error status: {0}")]
    ServerError(String),
}
