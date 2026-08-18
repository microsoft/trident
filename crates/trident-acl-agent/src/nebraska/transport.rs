//! The [`Transport`] abstraction over the HTTP round-trip to Nebraska.
//!
//! Abstracting the transport keeps the protocol logic in
//! [`Client`](crate::nebraska::Client) hermetically testable — unit tests inject
//! a canned transport and never touch the network — while the production path
//! uses a blocking `reqwest` client.
//!
//! # Blocking today; async is a non-breaking addition
//!
//! [`Transport`] is intentionally **synchronous**, matching the current agent
//! (which is otherwise sync and only enters a Tokio runtime for its Trident gRPC
//! call). A future async TAA that drives Trident over `tonic`/`tokio` must not
//! call a blocking HTTP client from within the async runtime, as that stalls the
//! executor.
//!
//! Supporting that does **not** require changing this API. Because
//! [`Client`](crate::nebraska::Client) is generic over the transport, an async
//! variant can be introduced *alongside* the sync one — a separate
//! `AsyncTransport` trait and a thin async client wrapper — without modifying or
//! breaking [`Transport`], [`ReqwestTransport`], or the existing `Client`
//! surface. The sync path is the right default now; the async path is additive
//! when a caller needs it. Until then, an async caller can also simply wrap a
//! sync call in `tokio::task::spawn_blocking`.

use std::time::Duration;

use url::Url;

use super::error::NebraskaError;

/// Default per-request timeout. Bounds every Omaha round-trip so a stalled
/// server turns into a retryable [`NebraskaError::Transport`] rather than
/// hanging indefinitely — which is what makes the bounded retry in
/// [`complete_after_reboot`](crate::nebraska::Client::complete_after_reboot)
/// actually bounded.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// Performs the HTTP POST of an Omaha request body and returns the response body.
///
/// Implementors should POST `body` to `endpoint` with an XML content type and
/// return the response text, mapping failures to [`NebraskaError::Transport`]
/// (connection failures) or [`NebraskaError::Http`] (non-success status).
pub trait Transport {
    /// POSTs `body` to `endpoint` and returns the response body as a string.
    fn post_xml(&self, endpoint: &Url, body: &[u8]) -> Result<String, NebraskaError>;
}

/// The default [`Transport`], backed by a blocking `reqwest` client.
///
/// Carries a per-request `timeout` (default [`DEFAULT_TIMEOUT`]) that is applied
/// to every request the transport issues.
#[derive(Debug)]
pub struct ReqwestTransport {
    client: reqwest::blocking::Client,
    timeout: Duration,
}

impl Default for ReqwestTransport {
    fn default() -> Self {
        Self {
            client: reqwest::blocking::Client::new(),
            timeout: DEFAULT_TIMEOUT,
        }
    }
}

impl ReqwestTransport {
    /// Creates a new transport with a default `reqwest` client and the default
    /// [`DEFAULT_TIMEOUT`] per-request timeout.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a transport that applies `timeout` to every request it issues.
    pub fn with_timeout(timeout: Duration) -> Self {
        Self {
            timeout,
            ..Self::default()
        }
    }
}

impl Transport for ReqwestTransport {
    fn post_xml(&self, endpoint: &Url, body: &[u8]) -> Result<String, NebraskaError> {
        // `reqwest::Error`'s `Display` embeds the request URL, which may carry an
        // Omaha secret; `without_url` strips it so the secret cannot reach a log
        // or a propagated error.
        self.client
            .post(endpoint.as_str())
            .header("Content-Type", "application/xml")
            .timeout(self.timeout)
            .body(body.to_vec())
            .send()
            .map_err(|e| NebraskaError::Transport(e.without_url().to_string()))?
            .error_for_status()
            .map_err(|e| NebraskaError::Http {
                status: e.status().map(|s| s.as_u16()),
                message: e.without_url().to_string(),
            })?
            .text()
            .map_err(|e| NebraskaError::Http {
                status: None,
                message: e.without_url().to_string(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{
        net::TcpListener,
        time::{Duration, Instant},
    };

    const TEST_TIMEOUT: Duration = Duration::from_millis(200);

    #[test]
    fn request_times_out_instead_of_hanging() {
        // A socket that accepts the connection but never answers: without a
        // per-request timeout this blocks forever, which would make the bounded
        // retry in `complete_after_reboot` unbounded in practice.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = Url::parse(&format!(
            "http://{}/v1/update/",
            listener.local_addr().unwrap()
        ))
        .unwrap();

        let started = Instant::now();
        let err = ReqwestTransport::with_timeout(TEST_TIMEOUT)
            .post_xml(&endpoint, b"<request/>")
            .unwrap_err();

        assert!(matches!(err, NebraskaError::Transport(_)), "got {err:?}");
        assert!(err.is_retryable(), "a timeout must be worth retrying");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "should have given up after {TEST_TIMEOUT:?}, took {:?}",
            started.elapsed()
        );
        // The endpoint path can carry an Omaha secret, so it must not survive
        // into the error text.
        assert!(!err.to_string().contains("/v1/update"), "{err}");
    }
}
