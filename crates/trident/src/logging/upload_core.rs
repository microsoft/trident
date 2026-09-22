//! Low-level upload primitives shared by [`super::background_uploader`] (an
//! unbounded queue used for real log forwarding, where losing queued data is
//! not acceptable) and [`super::telemetry_uploader`] (a bounded ring used for
//! best-effort Application Insights telemetry, where producers must never
//! block or fail on enqueue). Everything here is queue-strategy-agnostic: it
//! knows how to POST one [`UploadData`] and track per-origin backoff, and how
//! to spin up the dedicated OS thread + Tokio runtime that drives an
//! uploader's loop to completion -- but nothing about *how* items are
//! queued, which is the part that actually differs between the two
//! uploaders and is why each keeps its own queue type instead of sharing one
//! here.

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

/// A static HTTP client for background uploads.
///
/// Redirects are only followed while the redirected URL stays `https`.
/// Telemetry uploads (e.g. Application Insights) carry host identifiers,
/// so an HTTPS-only endpoint must never be silently downgraded to
/// plaintext via a 307/308 redirect to an `http://` URL -- reqwest's
/// default policy follows redirects of any scheme.
pub(super) static HTTP_ASYNC_CLIENT: LazyLock<Client> = LazyLock::new(|| {
    Client::builder()
        .redirect(Policy::custom(|attempt| {
            if attempt.url().scheme() == "https" {
                attempt.follow()
            } else {
                attempt.error("refusing to follow redirect to a non-https URL")
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
/// for each further consecutive failure (see [`OriginCooldown`]) up to
/// [`MAX_ORIGIN_COOLDOWN`]. Bounding the backoff instead of disabling the
/// origin outright means a healthy endpoint recovers on its own after a
/// transient blip (a momentary network hiccup, a brief server restart),
/// while a genuinely dead one is still backed off hard enough not to waste
/// effort retrying it constantly.
const BASE_ORIGIN_COOLDOWN: Duration = Duration::from_secs(30);

/// Upper bound on the exponential backoff described above.
const MAX_ORIGIN_COOLDOWN: Duration = Duration::from_secs(600);

/// Validates an upload response beyond a bare 2xx status, for callers whose
/// ingestion protocol can reject part of a request while still returning a
/// 2xx status (e.g. Application Insights' 206 Partial Success). Given the
/// response status and body, returns `Ok(())` if the upload should be
/// treated as a success, or `Err` (triggering the same retry/backoff path
/// as a network-level failure) otherwise.
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

/// Outcome of processing a single queued upload through [`attempt_upload`],
/// distinguishing "skipped, origin is cooling down" from "actually
/// attempted" (successfully or not). Used by the ring-backed loop (see
/// `telemetry_uploader::ring_upload_loop`) to decide whether to keep
/// draining during shutdown.
pub(super) enum AttemptOutcome {
    Attempted { succeeded: bool },
    SkippedCooldown,
}

/// Attempts a single queued upload, checking/updating `origin_cooldowns`.
/// Shared by both the unbounded and ring-backed loops so this cooldown/
/// backoff/response-validation logic has exactly one copy instead of it
/// drifting between two.
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
    // Treat non-2xx responses the same as a network-level failure: a
    // consumer (e.g. AppInsightsSender) may document that rejected
    // requests count as failures, so surface them here rather than
    // silently treating any response as success. `error_for_status()`
    // alone is not enough: it only rejects 4xx/5xx, so a 3xx (e.g. an
    // unexpected redirect the client never followed) would still be
    // reported as success. Explicitly require 2xx instead. A 2xx
    // status alone is still not sufficient for every caller: some
    // ingestion protocols (e.g. Application Insights) can return a
    // 2xx (206 Partial Success) while rejecting part or all of the
    // request body, so a caller-supplied `response_validator` gets
    // the final say when present.
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

/// Waits up to `deadline` for `handle` to finish, returning its result if it
/// does. `JoinHandle::join` has no built-in timeout, so this moves the
/// actual join onto a throwaway thread and applies the timeout via a
/// channel receive instead; if `deadline` elapses first, that throwaway
/// thread (and by extension whatever `handle` was waiting on) is
/// abandoned rather than awaited further.
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
        let failing = server.mock("POST", "/fail").with_status(500).expect(1).create();
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
    /// The HTTP client must refuse to follow a redirect whose target is not
    /// `https`, even though the redirect response itself succeeds -- an
    /// HTTPS-only telemetry endpoint must never be silently downgraded to
    /// plaintext by a 307/308 redirect to an `http://` URL.
    fn test_attempt_upload_rejects_redirect_to_non_https_url() {
        init_test_logging();

        let mut server = Server::new();
        let redirect_mock = server
            .mock("POST", "/redirect")
            .with_status(307)
            .with_header("location", "http://example.invalid/plaintext-upload")
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

        // The redirect response was received, but the client must not have
        // actually sent a request to the plaintext `http://example.invalid`
        // target -- there is no mock for it, so a follow would panic/error
        // out with a connection failure rather than silently succeeding.
        redirect_mock.assert();
    }

    #[test]
    /// Deliberately not exercised via a real uploader + network mock: a
    /// genuinely abandoned background thread would keep running past this
    /// test's own scope, in a process shared with every other test in the
    /// suite, risking exactly the kind of cross-test port/resource
    /// collisions a slow real HTTP mock invites under `cargo test`'s
    /// default parallelism. `join_with_deadline` is pure std-only plumbing
    /// (a thread + a timed channel receive), so testing it directly with a
    /// plain `thread::spawn` gives the same coverage without that risk.
    fn test_join_with_deadline_abandons_a_slow_thread() {
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let handle = std::thread::spawn(move || {
            // Blocks until the test explicitly releases it below, standing
            // in for a still-busy background uploader thread.
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

        // Unlike the real "abandon" scenario this stands in for, we can
        // cleanly unblock the spawned thread here, so it exits rather than
        // lingering for the rest of the test binary's process lifetime.
        let _ = release_tx.send(());
    }
}
