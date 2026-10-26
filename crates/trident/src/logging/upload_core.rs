//! Shared upload primitives for [`super::background_uploader`] and
//! [`super::telemetry_uploader`]. This module handles one POST at a time,
//! per-origin backoff, and the dedicated uploader thread/runtime; queueing
//! stays with each uploader because their delivery policies differ.

use std::{
    collections::HashMap,
    future::Future,
    sync::LazyLock,
    thread::{Builder, JoinHandle},
    time::{Duration, Instant},
};

use anyhow::{bail, Context, Error};
use log::error;
use reqwest::{redirect::Policy, Client};
use tokio::sync::oneshot;
use url::{Origin, Url};

/// Maximum redirect chain length. `Policy::custom` disables reqwest's
/// built-in cap, so this replicates it (reqwest's `Policy::default()`
/// also stops following after 10 redirects).
const MAX_REDIRECTS: usize = 10;

/// Decides whether a redirect should be followed, given whether the
/// original request started as `https` and the scheme of the redirect
/// target. Pulled out of the `Policy::custom` closure below so it can be
/// unit-tested directly (mockito only serves plain HTTP, so an HTTPS ->
/// HTTP downgrade can't be exercised end-to-end without a real TLS
/// endpoint).
///
/// Returns `Ok(())` to follow the redirect, or `Err(reason)` to refuse it.
fn redirect_decision(
    started_https: bool,
    target_scheme: &str,
    redirects_so_far: usize,
) -> Result<(), &'static str> {
    if started_https && target_scheme != "https" {
        Err("refusing to follow an HTTPS -> non-HTTPS redirect")
    } else if redirects_so_far >= MAX_REDIRECTS {
        Err("too many redirects")
    } else {
        Ok(())
    }
}

/// A static HTTP client for background uploads.
///
/// Redirects are followed unless they would downgrade an HTTPS request to
/// a non-HTTPS URL. Telemetry uploads (e.g. Application Insights) always
/// start as HTTPS and so stay protected, while HTTP endpoints (e.g.
/// logstream/tracestream forwarding, which this client is also used for)
/// may still redirect within HTTP -- reqwest's default policy would
/// otherwise follow redirects of any scheme, and a naive https-only check
/// would wrongly break those HTTP redirects.
pub(super) static HTTP_ASYNC_CLIENT: LazyLock<Client> = LazyLock::new(|| {
    Client::builder()
        .redirect(Policy::custom(|attempt| {
            let started_https = attempt
                .previous()
                .first()
                .is_some_and(|u| u.scheme() == "https");
            match redirect_decision(
                started_https,
                attempt.url().scheme(),
                attempt.previous().len(),
            ) {
                Ok(()) => attempt.follow(),
                Err(reason) => attempt.error(reason),
            }
        }))
        .build()
        .expect("failed to build HTTP_ASYNC_CLIENT")
});

/// The module path of the shared upload logic. Used to filter its own
/// error/debug logs out of log forwarding, so a failing endpoint can't
/// create a feedback loop of forwarding logs about itself failing to
/// forward.
pub(super) const BACKGROUND_LOG_MODULE: &str = module_path!();

/// Cooldown applied after an origin's first consecutive failure, doubled
/// on each further failure up to [`MAX_ORIGIN_COOLDOWN`]. This keeps dead
/// endpoints from being retried constantly while still allowing recovery.
const BASE_ORIGIN_COOLDOWN: Duration = Duration::from_secs(30);

/// Upper bound on the exponential backoff described above.
const MAX_ORIGIN_COOLDOWN: Duration = Duration::from_secs(600);

/// Validates a 2xx response for protocols that can still reject all or part
/// of a request (for example Application Insights' 206 Partial Success).
/// Return `Ok(())` to accept the upload, or `Err` to trigger normal retry/backoff.
pub(super) type ResponseValidator = fn(reqwest::StatusCode, &[u8]) -> Result<(), Error>;

/// Data to be uploaded.
pub(super) struct UploadData {
    pub(super) url: Url,
    pub(super) body: Vec<u8>,
    pub(super) timeout: Duration,
    /// Optional `Content-Type` header value to attach to the request.
    pub(super) content_type: Option<&'static str>,
    /// Optional response validator -- see [`ResponseValidator`]. When
    /// `None`, falls back to treating any 2xx status as success.
    pub(super) response_validator: Option<ResponseValidator>,
}

/// Per-origin backoff state, tracked across consecutive failed uploads to
/// the same origin. Reset (removed from the tracking map) as soon as an
/// upload to that origin succeeds again.
pub(super) struct OriginCooldown {
    /// success (or since tracking began).
    consecutive_failures: u32,
    /// Uploads to this origin are skipped until this instant.
    cooldown_until: Instant,
}

/// Per-origin backoff state, owned by a single uploader loop. The unbounded
/// and ring-backed loops each keep their own map: an origin failing on one
/// uploader has no effect on the other.
pub(super) type OriginCooldowns = HashMap<Origin, OriginCooldown>;

/// Computes the exponential backoff duration for the given number of
/// consecutive failures (1 for the first failure, 2 for the second, ...),
/// doubling from `BASE_ORIGIN_COOLDOWN` and clamped at
/// `MAX_ORIGIN_COOLDOWN`. Split out so the pure calculation is
/// independently testable without needing to simulate real time passing.
pub(super) fn backoff_for_failures(consecutive_failures: u32) -> Duration {
    // Cap the exponent well below any value that could overflow the
    // shift: MAX_ORIGIN_COOLDOWN already clamps the result, so this only
    // needs to be large enough to reach that clamp.
    let exponent = consecutive_failures.saturating_sub(1).min(16);
    BASE_ORIGIN_COOLDOWN
        .saturating_mul(1u32 << exponent)
        .min(MAX_ORIGIN_COOLDOWN)
}

/// Result of processing one queued upload through [`attempt_upload`].
/// Used by the telemetry ring loop to tell "skipped due to cooldown" from
/// an upload that was actually attempted.
pub(super) enum AttemptOutcome {
    Attempted { succeeded: bool },
    SkippedCooldown,
}

/// Attempts one queued upload and updates `origin_cooldowns`.
/// Shared by both uploader loops so cooldown/backoff behavior stays identical.
pub(super) async fn attempt_upload(
    origin_cooldowns: &mut OriginCooldowns,
    upload: UploadData,
) -> AttemptOutcome {
    let origin = upload.url.origin();

    if let Some(state) = origin_cooldowns.get(&origin) {
        if Instant::now() < state.cooldown_until {
            return AttemptOutcome::SkippedCooldown;
        }
    }

    let mut request = HTTP_ASYNC_CLIENT
        .post(upload.url.clone())
        .timeout(upload.timeout)
        .body(upload.body);
    if let Some(content_type) = upload.content_type {
        request = request.header(reqwest::header::CONTENT_TYPE, content_type);
    }
    // Treat anything other than a validated 2xx as a failure. `error_for_status()`
    // is not enough here because it would still accept 3xx responses, and some
    // ingestion protocols can reject data while returning 2xx. When present,
    // `response_validator` makes that final success/failure decision.
    let response_validator = upload.response_validator;
    let result: Result<(), Error> = match request.send().await {
        Ok(response) if response.status().is_success() => {
            let status = response.status();
            match response_validator {
                Some(validate) => match response.bytes().await {
                    Ok(body) => validate(status, &body),
                    Err(e) => Err(e.into()),
                },
                None => Ok(()),
            }
        }
        Ok(response) => Err(anyhow::anyhow!(
            "unexpected HTTP status {} from {}",
            response.status(),
            response.url()
        )),
        Err(e) => Err(e.into()),
    };

    match result {
        Ok(()) => {
            // Origin is healthy again: drop any backoff state so a
            // future failure starts from the base cooldown rather
            // than a previously-escalated one.
            origin_cooldowns.remove(&origin);
            AttemptOutcome::Attempted { succeeded: true }
        }
        Err(e) => {
            error!("Background upload failed: {e}");

            let consecutive_failures = origin_cooldowns
                .get(&origin)
                .map(|state| state.consecutive_failures)
                .unwrap_or(0)
                + 1;
            let cooldown = backoff_for_failures(consecutive_failures);
            let cooldown_until = Instant::now() + cooldown;

            origin_cooldowns.insert(
                origin.clone(),
                OriginCooldown {
                    consecutive_failures,
                    cooldown_until,
                },
            );

            error!(
                "Backing off uploads to server for {cooldown:?} (failure #{consecutive_failures}): {}",
                match origin {
                    Origin::Tuple(scheme, host, port) =>
                        format!("{}://{}:{}", scheme, host, port),
                    Origin::Opaque(_) => "[opaque origin]".to_string(),
                }
            );

            AttemptOutcome::Attempted { succeeded: false }
        }
    }
}

/// Spawns a dedicated OS thread running a single-threaded Tokio runtime
/// that drives `task` to completion. Shared by both uploaders so the
/// thread/runtime bootstrapping (and its readiness handshake) only needs to
/// be implemented once.
pub(super) fn spawn_uploader_thread(
    task: impl Future<Output = ()> + Send + 'static,
) -> Result<JoinHandle<()>, Error> {
    let (ready_tx, ready_rx) = oneshot::channel::<bool>();
    let handle = Builder::new()
        .name("background-uploader".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            let _ = ready_tx.send(runtime.is_ok());
            let runtime = match runtime {
                Ok(rt) => rt,
                Err(e) => {
                    eprintln!("Failed to create Tokio runtime for background uploader: {e}");
                    return;
                }
            };

            runtime.block_on(task);
        })
        .context("Failed to create background-uploader thread.")?;

    // Wait for the runtime to be ready
    match ready_rx.blocking_recv() {
        Ok(true) => Ok(handle),
        Ok(false) => bail!("Failed to create Tokio runtime for background uploader"),
        Err(e) => bail!("Background uploader thread terminated unexpectedly: {e}"),
    }
}

/// Waits up to `deadline` for `handle` to finish.
/// Since `JoinHandle::join` has no timeout, the join runs on a helper thread
/// and this call times out on the channel receive instead.
pub(super) fn join_with_deadline<T: Send + 'static>(
    handle: JoinHandle<T>,
    deadline: Duration,
) -> Result<std::thread::Result<T>, std::sync::mpsc::RecvTimeoutError> {
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let _ = Builder::new()
        .name("background-uploader-shutdown-watcher".into())
        .spawn(move || {
            let _ = done_tx.send(handle.join());
        });
    done_rx.recv_timeout(deadline)
}

#[cfg(test)]
mod tests {
    use super::*;

    use mockito::{Matcher, Server};

    fn init_test_logging() {
        let _ = env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init();
    }

    fn run_in_runtime(f: impl std::future::Future<Output = ()>) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(f);
    }

    fn mock_upload(url: Url, body: &str) -> UploadData {
        UploadData {
            url,
            body: body.as_bytes().to_vec(),
            timeout: Duration::from_secs(2),
            content_type: None,
            response_validator: None,
        }
    }

    #[test]
    /// The backoff schedule should start at the base cooldown, double with
    /// each consecutive failure, and clamp at the configured maximum
    /// instead of growing unbounded (or overflowing) for a long-dead
    /// origin that keeps failing indefinitely.
    fn test_backoff_for_failures_doubles_and_clamps() {
        assert_eq!(backoff_for_failures(1), BASE_ORIGIN_COOLDOWN);
        assert_eq!(backoff_for_failures(2), BASE_ORIGIN_COOLDOWN * 2);
        assert_eq!(backoff_for_failures(3), BASE_ORIGIN_COOLDOWN * 4);
        assert_eq!(backoff_for_failures(100), MAX_ORIGIN_COOLDOWN);
        assert_eq!(backoff_for_failures(u32::MAX), MAX_ORIGIN_COOLDOWN);
    }

    #[test]
    /// A queued upload results in a single HTTP POST with the given body.
    fn test_attempt_upload_sends_post_request() {
        init_test_logging();

        let mut server = Server::new();
        let body = "hello-attempt-upload";
        let mock = server
            .mock("POST", "/upload")
            .match_body(Matcher::Exact(body.to_string()))
            .with_status(200)
            .expect(1)
            .create();

        let url = Url::parse(&server.url()).unwrap().join("/upload").unwrap();
        run_in_runtime(async {
            let mut cooldowns = OriginCooldowns::new();
            let outcome = attempt_upload(&mut cooldowns, mock_upload(url, body)).await;
            assert!(matches!(
                outcome,
                AttemptOutcome::Attempted { succeeded: true }
            ));
        });
        mock.assert();
    }

    #[test]
    /// Once an origin has failed, further uploads to that same origin are
    /// skipped (not attempted at all) until its cooldown expires.
    fn test_attempt_upload_skips_within_cooldown_after_failure() {
        init_test_logging();

        let mut server = Server::new();
        let failing = server
            .mock("POST", "/fail")
            .with_status(500)
            .expect(1)
            .create();
        let should_not_hit = server
            .mock("POST", "/upload")
            .with_status(200)
            .expect(0)
            .create();

        run_in_runtime(async {
            let mut cooldowns = OriginCooldowns::new();
            let fail_url = Url::parse(&server.url()).unwrap().join("/fail").unwrap();
            let outcome = attempt_upload(&mut cooldowns, mock_upload(fail_url, "will-fail")).await;
            assert!(matches!(
                outcome,
                AttemptOutcome::Attempted { succeeded: false }
            ));

            // Same origin, still within the cooldown window: should be
            // skipped outright rather than attempted.
            let upload_url = Url::parse(&server.url()).unwrap().join("/upload").unwrap();
            let outcome =
                attempt_upload(&mut cooldowns, mock_upload(upload_url, "should-be-skipped")).await;
            assert!(matches!(outcome, AttemptOutcome::SkippedCooldown));
        });

        failing.assert();
        should_not_hit.assert();
    }

    #[test]
    /// A 3xx response (which `error_for_status()` alone would treat as
    /// success) must still be handled as a failure.
    fn test_attempt_upload_redirect_status_is_treated_as_failure() {
        init_test_logging();

        let mut server = Server::new();
        let redirect_mock = server
            .mock("POST", "/redirect")
            .with_status(302)
            .expect(1)
            .create();

        let url = Url::parse(&server.url())
            .unwrap()
            .join("/redirect")
            .unwrap();
        run_in_runtime(async {
            let mut cooldowns = OriginCooldowns::new();
            let outcome = attempt_upload(&mut cooldowns, mock_upload(url, "redirect-me")).await;
            assert!(matches!(
                outcome,
                AttemptOutcome::Attempted { succeeded: false }
            ));
        });

        redirect_mock.assert();
    }

    #[test]
    /// `redirect_decision` must refuse a redirect that downgrades an
    /// HTTPS request to a non-HTTPS target. This is the actual regression
    /// coverage for the HTTPS-downgrade protection: mockito only serves
    /// plain HTTP, so this can't be exercised end-to-end with a real
    /// request (see `test_attempt_upload_follows_http_to_http_redirect`
    /// below for the end-to-end HTTP -> HTTP case this client also needs
    /// to support).
    fn test_redirect_decision_rejects_https_to_http_downgrade() {
        assert!(redirect_decision(true, "http", 0).is_err());
    }

    #[test]
    /// An HTTPS request redirecting to another HTTPS target is unaffected.
    fn test_redirect_decision_allows_https_to_https() {
        assert!(redirect_decision(true, "https", 0).is_ok());
    }

    #[test]
    /// `redirect_decision` caps the redirect chain length, since
    /// `Policy::custom` disables reqwest's own default limit.
    fn test_redirect_decision_enforces_redirect_limit() {
        assert!(redirect_decision(false, "http", MAX_REDIRECTS).is_err());
        assert!(redirect_decision(false, "http", MAX_REDIRECTS - 1).is_ok());
    }

    #[test]
    /// End-to-end: an HTTP -> HTTP redirect must actually be followed
    /// (unlike the old https-only policy this replaced, which would have
    /// wrongly rejected this and broken supported HTTP logstream/
    /// tracestream redirects).
    fn test_attempt_upload_follows_http_to_http_redirect() {
        init_test_logging();

        let mut server = Server::new();
        let redirect_mock = server
            .mock("POST", "/redirect")
            .with_status(307)
            .with_header("location", "/redirect-target")
            .expect(1)
            .create();
        let target_mock = server.mock("POST", "/redirect-target").expect(1).create();

        let url = Url::parse(&server.url())
            .unwrap()
            .join("/redirect")
            .unwrap();

        run_in_runtime(async {
            let mut cooldowns = OriginCooldowns::new();
            let outcome = attempt_upload(&mut cooldowns, mock_upload(url, "redirect-me")).await;
            assert!(matches!(
                outcome,
                AttemptOutcome::Attempted { succeeded: true }
            ));
        });

        redirect_mock.assert();
        target_mock.assert();
    }

    #[test]
    /// Test this directly with `thread::spawn` rather than a real uploader:
    /// abandoning a live uploader thread would risk cross-test interference
    /// under `cargo test`'s default parallelism.
    fn test_join_with_deadline_abandons_a_slow_thread() {
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let handle = std::thread::spawn(move || {
            // Stand in for a still-busy background uploader thread.
            let _ = release_rx.recv();
        });

        let start = Instant::now();
        let result = join_with_deadline(handle, Duration::from_millis(50));
        let elapsed = start.elapsed();

        assert!(
            result.is_err(),
            "join_with_deadline should report a timeout, not a completed join"
        );
        assert!(
            elapsed < Duration::from_secs(1),
            "join_with_deadline should return near its deadline, not block on \
             the still-running thread; took {elapsed:?}"
        );

        // Cleanly release the stand-in thread so it does not linger for the
        // rest of the test process.
        let _ = release_tx.send(());
    }
}
