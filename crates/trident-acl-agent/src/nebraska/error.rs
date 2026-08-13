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
///   not an error.
/// - An unrecognised status string never fails parsing; it is preserved in an
///   `Other` variant.
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

    /// The Nebraska server returned a non-success HTTP status. Carries the
    /// status code (when known) so retry logic can distinguish transient
    /// server errors from permanent ones.
    #[error("Nebraska returned an HTTP error: {message}")]
    Http {
        /// The HTTP status code, if the failure carried one.
        status: Option<u16>,
        /// The underlying error message.
        message: String,
    },

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

    /// [`Client::complete_after_reboot`](crate::nebraska::Client::complete_after_reboot)
    /// sent the completion report, but Nebraska still reports
    /// [`CheckOutcome::UpdateInProgress`](crate::nebraska::CheckOutcome::UpdateInProgress)
    /// rather than reflecting completion. This is retryable for the same
    /// reason a transport failure is: losing the terminal event wedges the
    /// instance permanently, so the caller must keep retrying rather than
    /// treat an unacknowledged completion as success.
    #[error("Nebraska still reports the update in progress after completion was reported")]
    CompletionNotAcknowledged,
}

impl NebraskaError {
    /// Whether this error is transient and the request is worth retrying.
    ///
    /// This distinction matters most for the post-reboot completion report: the
    /// first network call after a reboot routinely fails while DNS and routing
    /// settle, and losing the terminal event **wedges the instance permanently**
    /// (there is no server-side self-heal from that state). A caller retrying
    /// that report should loop while `is_retryable()` holds (with a bounded
    /// backoff), and stop on a permanent error rather than spinning on it — the
    /// inverse mistake (retrying a permanent failure) is just as damaging.
    ///
    /// Transport failures are always transient. HTTP failures are transient only
    /// for server-side 5xx errors *other than* `501 Not Implemented`: Nebraska
    /// returns 501 when an Omaha secret is configured and the client's URL lacks
    /// it, which is a permanent client misconfiguration that must not be retried.
    /// 4xx are likewise permanent. All protocol-level failures (serialization,
    /// parse, unexpected response, server error status, invalid request) are
    /// permanent.
    pub fn is_retryable(&self) -> bool {
        match self {
            NebraskaError::Transport(_) => true,
            // A 5xx other than 501 is a transient server/infrastructure error;
            // 4xx and 501 are permanent. A missing status (e.g. a body-read
            // failure) is treated as transient.
            NebraskaError::Http {
                status: Some(code), ..
            } => (500..600).contains(code) && *code != 501,
            NebraskaError::Http { status: None, .. } => true,
            NebraskaError::InvalidRequest(_)
            | NebraskaError::Serialize(_)
            | NebraskaError::Parse(_)
            | NebraskaError::UnexpectedResponse(_)
            | NebraskaError::ServerError(_) => false,
            NebraskaError::CompletionNotAcknowledged => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn http(status: Option<u16>) -> NebraskaError {
        NebraskaError::Http {
            status,
            message: "http error".to_string(),
        }
    }

    #[test]
    fn transient_errors_are_retryable() {
        assert!(NebraskaError::Transport("connection refused".into()).is_retryable());
        assert!(http(Some(502)).is_retryable());
        assert!(http(Some(503)).is_retryable());
        assert!(http(None).is_retryable());
    }

    #[test]
    fn permanent_http_errors_are_not_retryable() {
        // 501 = Nebraska rejecting a wrong/missing Omaha secret in the URL.
        assert!(!http(Some(501)).is_retryable());
        // 4xx are client errors and permanent.
        assert!(!http(Some(400)).is_retryable());
        assert!(!http(Some(404)).is_retryable());
    }

    #[test]
    fn permanent_errors_are_not_retryable() {
        assert!(!NebraskaError::InvalidRequest("bad".into()).is_retryable());
        assert!(!NebraskaError::Serialize("x".into()).is_retryable());
        assert!(!NebraskaError::Parse("x".into()).is_retryable());
        assert!(!NebraskaError::UnexpectedResponse("x".into()).is_retryable());
        assert!(!NebraskaError::ServerError("error-osnotsupported".into()).is_retryable());
    }

    #[test]
    fn completion_not_acknowledged_is_retryable() {
        assert!(NebraskaError::CompletionNotAcknowledged.is_retryable());
    }
}
