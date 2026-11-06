//! The [`Transport`] abstraction over the HTTP round-trip to Nebraska.
//!
//! Abstracting the transport keeps the protocol logic in
//! [`Client`](crate::nebraska::Client) hermetically testable — unit tests inject
//! a canned transport and never touch the network — while the production path
//! uses a blocking `reqwest` client.
//!
//! # Blocking today; async is a non-breaking addition
//!
//! [`Transport`] is intentionally **synchronous**, matching the current caller
//! (which is otherwise sync and only enters a Tokio runtime for an unrelated
//! gRPC call). A future async caller must not call a blocking HTTP client from
//! within an async runtime, as that stalls the executor.
//!
//! Supporting that does **not** require changing this API. Because
//! [`Client`](crate::nebraska::Client) is generic over the transport, an async
//! variant can be introduced *alongside* the sync one — a separate
//! `AsyncTransport` trait and a thin async client wrapper — without modifying or
//! breaking [`Transport`], [`ReqwestTransport`], or the existing `Client`
//! surface. The sync path is the right default now; the async path is additive
//! when a caller needs it. Until then, an async caller can also simply wrap a
//! sync call in `tokio::task::spawn_blocking`.

use std::time::{Duration, Instant};

use log::trace;
use reqwest::header::{COOKIE, SET_COOKIE};
use url::Url;

use super::{client::redacted, error::NebraskaError};

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
    /// POSTs `body` to `endpoint`, optionally carrying `cookie` as a
    /// `Cookie:` header, and returns the response body as a string together
    /// with any `Cookie:`-header-ready value derived from the response's
    /// `Set-Cookie` header(s) (`None` if the response set no cookie at all).
    ///
    /// This exists for Nebraska's load-balancer affinity cookie (e.g. an
    /// App-Service-style `ASLBSA` cookie): Nebraska issues one on any request
    /// that doesn't already carry it, and expects it echoed back on every
    /// later request. See [`Client`](crate::nebraska::Client), which owns
    /// the cookie across calls.
    fn post_xml(
        &self,
        endpoint: &Url,
        body: &[u8],
        cookie: Option<&str>,
    ) -> Result<(String, Option<String>), NebraskaError>;
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
    fn post_xml(
        &self,
        endpoint: &Url,
        body: &[u8],
        cookie: Option<&str>,
    ) -> Result<(String, Option<String>), NebraskaError> {
        let started = Instant::now();
        trace!(
            "Nebraska HTTP POST to '{}': request_bytes={} timeout_ms={}",
            redacted(endpoint),
            body.len(),
            self.timeout.as_millis()
        );
        let mut request = self
            .client
            .post(endpoint.as_str())
            .header("Content-Type", "application/xml")
            .timeout(self.timeout);
        if let Some(cookie) = cookie {
            request = request.header(COOKIE, cookie);
        }

        // `reqwest::Error`'s `Display` embeds the request URL, which may carry an
        // Omaha secret; `without_url` strips it so the secret cannot reach a log
        // or a propagated error.
        let response = request.body(body.to_vec()).send().map_err(|e| {
            let message = e.without_url().to_string();
            trace!(
                "Nebraska HTTP POST failed: elapsed_ms={} error={message}",
                started.elapsed().as_millis()
            );
            NebraskaError::Transport(message)
        })?;
        trace!(
            "Nebraska HTTP response: status={} headers_elapsed_ms={} content_length={:?}",
            response.status().as_u16(),
            started.elapsed().as_millis(),
            response.content_length()
        );

        // Captured before the status check: a cookie set alongside a
        // non-2xx response is not propagated today (the error path below
        // returns no cookie), but the success path - by far the common
        // case - must not miss one.
        let set_cookie = extract_cookie(&response);

        let text = response
            .error_for_status()
            .map_err(|e| NebraskaError::Http {
                status: e.status().map(|s| s.as_u16()),
                message: e.without_url().to_string(),
            })?
            .text()
            .map_err(|e| NebraskaError::Http {
                status: None,
                message: e.without_url().to_string(),
            });
        match &text {
            Ok(body) => trace!(
                "Nebraska HTTP response body: bytes={} total_elapsed_ms={}",
                body.len(),
                started.elapsed().as_millis()
            ),
            Err(error) => trace!(
                "Nebraska HTTP response body failed: total_elapsed_ms={} error={error}",
                started.elapsed().as_millis()
            ),
        }
        text.map(|body| (body, set_cookie))
    }
}

/// Builds a `Cookie:`-header-ready value from a response's `Set-Cookie`
/// header(s), or `None` if it carried none.
///
/// Nebraska's affinity cookie can come as more than one `Set-Cookie` header
/// (e.g. an App-Service-style `ASLBSA`/`ASLBSACORS` pair); only the
/// `name=value` portion before each header's first `;` belongs in an
/// outgoing `Cookie:` header, so attributes like `Path`/`Secure`/`Max-Age`
/// are deliberately dropped here. Multiple cookies are joined with `; `.
fn extract_cookie(response: &reqwest::blocking::Response) -> Option<String> {
    let pairs: Vec<String> = response
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .filter_map(|raw| raw.split(';').next())
        .map(str::trim)
        .filter(|pair| !pair.is_empty())
        .map(str::to_string)
        .collect();

    if pairs.is_empty() {
        None
    } else {
        Some(pairs.join("; "))
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
    fn http_responses_preserve_body_and_error_status() {
        for status in [200, 503] {
            let mut server = mockito::Server::new();
            let mock = server
                .mock("POST", "/v1/update/")
                .match_query(mockito::Matcher::Any)
                .match_header("content-type", "application/xml")
                .match_body("<request/>")
                .with_status(status)
                .with_body("<response/>")
                .create();
            let endpoint =
                Url::parse(&format!("{}/v1/update/?secret=hunter2", server.url())).unwrap();
            let result = ReqwestTransport::new().post_xml(&endpoint, b"<request/>", None);
            if status == 200 {
                assert_eq!(result.unwrap().0, "<response/>");
            } else {
                let error = result.unwrap_err();
                assert!(matches!(
                    error,
                    NebraskaError::Http {
                        status: Some(503),
                        ..
                    }
                ));
                assert!(!error.to_string().contains("hunter2"));
                assert!(!error.to_string().contains("/v1/update"));
            }
            mock.assert();
        }
    }

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
            .post_xml(&endpoint, b"<request/>", None)
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

    #[test]
    fn post_xml_sends_cookie_header_when_provided() {
        let mut server = mockito::Server::new();
        let mock = server
            .mock("POST", "/")
            .match_header("cookie", "ASLBSA=abc123")
            .with_status(200)
            .with_body("<response/>")
            .expect(1)
            .create();

        let (text, set_cookie) = ReqwestTransport::new()
            .post_xml(
                &Url::parse(&server.url()).unwrap(),
                b"<request/>",
                Some("ASLBSA=abc123"),
            )
            .unwrap();

        mock.assert();
        assert_eq!(text, "<response/>");
        assert_eq!(set_cookie, None);
    }

    #[test]
    fn post_xml_omits_cookie_header_when_none_stored_yet() {
        let mut server = mockito::Server::new();
        let mock = server
            .mock("POST", "/")
            .match_header("cookie", mockito::Matcher::Missing)
            .with_status(200)
            .with_body("<response/>")
            .expect(1)
            .create();

        ReqwestTransport::new()
            .post_xml(&Url::parse(&server.url()).unwrap(), b"<request/>", None)
            .unwrap();

        mock.assert();
    }

    #[test]
    fn post_xml_captures_set_cookie_from_response() {
        let mut server = mockito::Server::new();
        let mock = server
            .mock("POST", "/")
            .with_status(200)
            // Real affinity cookies carry attributes (Path/Secure/...); only
            // the name=value part should survive into the returned cookie.
            .with_header("set-cookie", "ASLBSA=abc123; path=/; secure; httponly")
            .with_body("<response/>")
            .create();

        let (_, set_cookie) = ReqwestTransport::new()
            .post_xml(&Url::parse(&server.url()).unwrap(), b"<request/>", None)
            .unwrap();

        mock.assert();
        assert_eq!(set_cookie.as_deref(), Some("ASLBSA=abc123"));
    }

    #[test]
    fn post_xml_joins_multiple_set_cookie_headers() {
        let mut server = mockito::Server::new();
        let mock = server
            .mock("POST", "/")
            .with_status(200)
            .with_header("set-cookie", "ASLBSA=abc123; path=/")
            .with_header("set-cookie", "ASLBSACORS=def456; path=/")
            .with_body("<response/>")
            .create();

        let (_, set_cookie) = ReqwestTransport::new()
            .post_xml(&Url::parse(&server.url()).unwrap(), b"<request/>", None)
            .unwrap();

        mock.assert();
        assert_eq!(
            set_cookie.as_deref(),
            Some("ASLBSA=abc123; ASLBSACORS=def456")
        );
    }

    #[test]
    fn post_xml_returns_none_when_response_sets_no_cookie() {
        let mut server = mockito::Server::new();
        let mock = server
            .mock("POST", "/")
            .with_status(200)
            .with_body("<response/>")
            .create();

        let (_, set_cookie) = ReqwestTransport::new()
            .post_xml(&Url::parse(&server.url()).unwrap(), b"<request/>", None)
            .unwrap();

        mock.assert();
        assert_eq!(set_cookie, None);
    }
}
