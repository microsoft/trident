use std::{
    collections::{HashMap, VecDeque},
    future::Future,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, LazyLock, Mutex, Weak,
    },
    thread::{Builder, JoinHandle},
    time::{Duration, Instant},
};

use anyhow::{bail, Context, Error};
use log::{debug, error};
use reqwest::{redirect::Policy, Client};
use tokio::sync::{
    mpsc::{self, UnboundedReceiver, UnboundedSender, WeakUnboundedSender},
    oneshot, Notify,
};
use url::{Origin, Url};

/// A static HTTP client for background uploads.
///
/// Redirects are only followed while the redirected URL stays `https`.
/// Telemetry uploads (e.g. Application Insights) carry host identifiers,
/// so an HTTPS-only endpoint must never be silently downgraded to
/// plaintext via a 307/308 redirect to an `http://` URL -- reqwest's
/// default policy follows redirects of any scheme.
static HTTP_ASYNC_CLIENT: LazyLock<Client> = LazyLock::new(|| {
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

/// The module path of the background uploader. Can be used for filtering logs.
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
pub(crate) type ResponseValidator = fn(reqwest::StatusCode, &[u8]) -> Result<(), Error>;

/// Data to be uploaded by the background uploader.
struct UploadData {
    url: Url,
    body: Vec<u8>,
    timeout: Duration,
    /// Optional `Content-Type` header value to attach to the request.
    content_type: Option<&'static str>,
    /// Optional response validator -- see [`ResponseValidator`]. When
    /// `None`, falls back to treating any 2xx status as success.
    response_validator: Option<ResponseValidator>,
}

/// Per-origin backoff state, tracked across consecutive failed uploads to
/// the same origin. Reset (removed from the tracking map) as soon as an
/// upload to that origin succeeds again.
struct OriginCooldown {
    /// Consecutive failures observed for this origin since its last
    /// success (or since tracking began).
    consecutive_failures: u32,
    /// Uploads to this origin are skipped until this instant.
    cooldown_until: Instant,
}

/// Computes the exponential backoff duration for the given number of
/// consecutive failures (1 for the first failure, 2 for the second, ...),
/// doubling from `BASE_ORIGIN_COOLDOWN` and clamped at
/// `MAX_ORIGIN_COOLDOWN`. Split out from `upload_loop` so the pure
/// calculation is independently testable without needing to simulate real
/// time passing.
fn backoff_for_failures(consecutive_failures: u32) -> Duration {
    // Cap the exponent well below any value that could overflow the
    // shift: MAX_ORIGIN_COOLDOWN already clamps the result, so this only
    // needs to be large enough to reach that clamp.
    let exponent = consecutive_failures.saturating_sub(1).min(16);
    BASE_ORIGIN_COOLDOWN
        .saturating_mul(1u32 << exponent)
        .min(MAX_ORIGIN_COOLDOWN)
}

/// Outcome of processing a single queued upload through
/// [`attempt_upload`], distinguishing "skipped, origin is cooling down"
/// from "actually attempted" (successfully or not). Used by
/// `ring_upload_loop` to decide whether to keep draining during shutdown
/// -- see there.
enum AttemptOutcome {
    Attempted { succeeded: bool },
    SkippedCooldown,
}

/// Attempts a single queued upload, checking/updating `origin_cooldowns`
/// exactly as `upload_loop` always has. Split out so both the unbounded
/// and ring-backed loops share one copy of the cooldown/backoff/response-
/// validation logic instead of it drifting between two copies.
async fn attempt_upload(
    origin_cooldowns: &mut HashMap<Origin, OriginCooldown>,
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

/// A bounded, non-blocking FIFO queue of pending uploads: pushing past
/// `capacity` evicts the oldest not-yet-attempted entry instead of
/// blocking the producer or growing without bound. Used for telemetry --
/// see `BackgroundUploader::new_ring` -- where producers (a
/// `tracing_subscriber::Layer` callback) must never block or fail on
/// enqueue, ruling out a backpressured bounded channel (whose `send()`
/// either awaits or rejects when full, neither of which is acceptable
/// here), and where a genuinely *failing* endpoint is already
/// self-limiting via `attempt_upload`'s per-origin cooldown, but a merely
/// *slow* one is not: nothing previously capped how large the backlog (or
/// its memory) could grow while such an endpoint kept the loop busy.
///
/// The `Mutex<VecDeque<_>>` critical section here is a brief, O(1),
/// always-progressing push/pop -- categorically different from the
/// backpressure this exists to avoid, which can block a producer for as
/// long as the *consumer's* slow I/O takes (up to `attempt_upload`'s own
/// request timeout).
struct TelemetryRing {
    queue: Mutex<VecDeque<UploadData>>,
    capacity: usize,
    /// Woken on every push, and once more by `signal_stop`, so `pop`'s
    /// wait can't outlive both a) new data arriving and b) shutdown being
    /// requested with nothing left to push.
    notify: Notify,
    /// Set by `shutdown_with_deadline`/`Drop` before the join wait begins.
    /// `ring_upload_loop` only consults this to decide whether to keep
    /// draining *after* a failed upload -- during normal operation
    /// (before this is set), a failure is handled exactly like the
    /// unbounded loop's (logged, backed off, keep going).
    stop_requested: AtomicBool,
}

impl TelemetryRing {
    fn new(capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            queue: Mutex::new(VecDeque::with_capacity(capacity)),
            capacity,
            notify: Notify::new(),
            stop_requested: AtomicBool::new(false),
        })
    }

    /// Enqueues `item`, or refuses it once shutdown has been signaled.
    ///
    /// The stop check and the enqueue happen under the same `queue` lock
    /// that `signal_stop` and `pop` also take, so a handle can never
    /// observe `stop_requested` as unset, then have its item land in the
    /// queue *after* `pop` has already decided (under that same lock)
    /// that the queue is empty and shutdown is complete -- which would
    /// otherwise let `push` return `Ok(())` for an item that is silently
    /// dropped instead of ever being processed.
    fn push(&self, item: UploadData) -> Result<(), Error> {
        let mut queue = self.queue.lock().unwrap();
        if self.stop_requested.load(Ordering::Acquire) {
            bail!("Background uploader has been shut down");
        }
        if queue.len() >= self.capacity {
            queue.pop_front();
        }
        queue.push_back(item);
        drop(queue);
        self.notify.notify_one();
        Ok(())
    }

    /// Waits for and returns the next item, or `None` once shutdown has
    /// been requested and the queue has been fully drained.
    async fn pop(&self) -> Option<UploadData> {
        loop {
            {
                let mut queue = self.queue.lock().unwrap();
                if let Some(item) = queue.pop_front() {
                    return Some(item);
                }
                if self.stop_requested.load(Ordering::Acquire) {
                    return None;
                }
            }
            self.notify.notified().await;
        }
    }

    /// Signals shutdown under the same `queue` lock `push`/`pop` use, so
    /// no concurrent `push` can slip an item past this point without
    /// observing `stop_requested` -- see `push`'s doc comment.
    fn signal_stop(&self) {
        let _queue = self.queue.lock().unwrap();
        self.stop_requested.store(true, Ordering::Release);
        drop(_queue);
        self.notify.notify_one();
    }
}

/// The two queueing strategies `BackgroundUploader` supports -- see
/// [`BackgroundUploader::new`] (unbounded, for log forwarding) and
/// [`BackgroundUploader::new_ring`] (bounded ring, for telemetry).
enum UploaderInner {
    Unbounded(UnboundedSender<UploadData>, JoinHandle<()>),
    Ring(Arc<TelemetryRing>, JoinHandle<()>),
}

/// A background uploader that sends log data to a remote server asynchronously.
///
/// When dropped it will finish any pending uploads and shut down the background
/// thread.
pub struct BackgroundUploader {
    inner: Option<UploaderInner>,
}

impl BackgroundUploader {
    /// Creates a new background uploader with an unbounded pending-upload
    /// queue. Used for real log forwarding, where losing queued data is
    /// not acceptable.
    pub fn new() -> Result<Self, Error> {
        let (sender, receiver) = mpsc::unbounded_channel();
        let handle = Self::start_upload_task(receiver)?;
        Ok(Self {
            inner: Some(UploaderInner::Unbounded(sender, handle)),
        })
    }

    /// Like [`Self::new`], but bounds the pending-upload queue to at most
    /// `capacity` entries via [`TelemetryRing`] instead of growing
    /// without bound. Intended for telemetry: producers must never block
    /// or fail on enqueue, and low telemetry volume plus the daemon's
    /// short-lived-between-sparse-requests lifecycle make actual
    /// unbounded growth unlikely in practice, but capping it is cheap
    /// insurance against a slow-but-succeeding endpoint on a busier or
    /// longer-lived process than typically expected.
    pub fn new_ring(capacity: usize) -> Result<Self, Error> {
        let ring = TelemetryRing::new(capacity);
        let handle = Self::start_ring_upload_task(ring.clone())?;
        Ok(Self {
            inner: Some(UploaderInner::Ring(ring, handle)),
        })
    }

    /// Gets a handle to send data to the uploader. Returns `None` if the uploader has been shut down.
    pub fn get_handle(&self) -> Option<BackgroundUploadHandle> {
        Some(BackgroundUploadHandle {
            inner: match self.inner.as_ref()? {
                UploaderInner::Unbounded(sender, _) => HandleInner::Unbounded(sender.downgrade()),
                UploaderInner::Ring(ring, _) => HandleInner::Ring(Arc::downgrade(ring)),
            },
        })
    }

    /// Starts a new thread with a Tokio runtime to handle uploads from the
    /// unbounded channel.
    fn start_upload_task(receiver: UnboundedReceiver<UploadData>) -> Result<JoinHandle<()>, Error> {
        Self::spawn_uploader_thread(Self::upload_loop(receiver))
    }

    /// Like [`Self::start_upload_task`], but for the ring-backed loop.
    fn start_ring_upload_task(ring: Arc<TelemetryRing>) -> Result<JoinHandle<()>, Error> {
        Self::spawn_uploader_thread(Self::ring_upload_loop(ring))
    }

    /// Spawns a dedicated OS thread running a single-threaded Tokio
    /// runtime that drives `task` to completion. Shared by both
    /// `start_upload_task` and `start_ring_upload_task` so the thread/
    /// runtime bootstrapping (and its readiness handshake) only needs to
    /// be implemented once.
    fn spawn_uploader_thread(
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

    /// The main upload loop that processes incoming upload requests from
    /// the unbounded channel. Runs until the channel is closed (all
    /// senders dropped) and fully drained.
    async fn upload_loop(mut receiver: UnboundedReceiver<UploadData>) {
        let mut origin_cooldowns: HashMap<Origin, OriginCooldown> = HashMap::new();

        while let Some(upload) = receiver.recv().await {
            attempt_upload(&mut origin_cooldowns, upload).await;
        }

        debug!("Background uploader loop has exited");
    }

    /// Like [`Self::upload_loop`], but for a [`TelemetryRing`]-backed
    /// queue. Behaves identically to `upload_loop` during normal
    /// operation (a failed upload is logged, backed off via
    /// `origin_cooldowns`, and the loop keeps going) -- the two loops
    /// diverge only once shutdown has been requested (`ring.pop()`
    /// returning items after `signal_stop` was called): from that point,
    /// the *first* failed upload stops the loop outright, abandoning
    /// whatever is left queued, rather than continuing to drain it.
    ///
    /// This is a deliberate choice over draining the entire backlog
    /// unconditionally: `shutdown_with_deadline`'s external deadline
    /// already hard-caps how long anything waits on this loop, so a
    /// large ring full of slow-but-succeeding uploads is not a
    /// correctness risk either way -- but stopping on first failure
    /// avoids spending that bounded shutdown window on retries to an
    /// endpoint that's *also* now failing, in favor of exiting promptly.
    async fn ring_upload_loop(ring: Arc<TelemetryRing>) {
        let mut origin_cooldowns: HashMap<Origin, OriginCooldown> = HashMap::new();

        while let Some(upload) = ring.pop().await {
            let draining = ring.stop_requested.load(Ordering::Acquire);
            let outcome = attempt_upload(&mut origin_cooldowns, upload).await;
            if draining && matches!(outcome, AttemptOutcome::Attempted { succeeded: false }) {
                debug!(
                    "Background ring uploader stopping drain after a failed upload during shutdown"
                );
                break;
            }
        }

        debug!("Background ring uploader loop has exited");
    }
}

impl BackgroundUploader {
    /// Signals the uploader to shut down, waiting up to `deadline` for its
    /// background thread to drain whatever is already queued and exit.
    ///
    /// `Drop`'s own shutdown (used when this isn't called explicitly) waits
    /// unboundedly: `origin_cooldowns` (see `start_upload_task`) bounds the
    /// wait for an origin that outright *fails*, since further requests to
    /// it within its current backoff window are skipped outright, but a
    /// slow-but-successful endpoint is not bounded that way -- every queued
    /// request still gets its own attempt, each up to that request's own
    /// timeout, so draining a large backlog could still take a while.
    /// Callers for whom that matters (telemetry in particular: "must never
    /// meaningfully delay Trident's actual work" is a stated design goal
    /// here) should call this explicitly instead of just letting the value
    /// drop.
    ///
    /// If `deadline` elapses first, the background thread is abandoned
    /// (its remaining queued requests may still complete before the
    /// process actually exits, but this call returns without waiting
    /// further for them).
    pub fn shutdown_with_deadline(mut self, deadline: Duration) {
        let Some(inner) = self.inner.take() else {
            return;
        };

        let handle = match inner {
            UploaderInner::Unbounded(sender, handle) => {
                drop(sender);
                handle
            }
            UploaderInner::Ring(ring, handle) => {
                ring.signal_stop();
                handle
            }
        };

        match join_with_deadline(handle, deadline) {
            Ok(Ok(())) => debug!("Background uploader shut down"),
            Ok(Err(e)) => error!("Background uploader thread panicked: {:?}", e),
            Err(_) => {
                debug!("Background uploader did not shut down within {deadline:?}; abandoning it")
            }
        }
    }
}

/// Waits up to `deadline` for `handle` to finish, returning its result if it
/// does. `JoinHandle::join` has no built-in timeout, so this moves the
/// actual join onto a throwaway thread and applies the timeout via a
/// channel receive instead; if `deadline` elapses first, that throwaway
/// thread (and by extension whatever `handle` was waiting on) is
/// abandoned rather than awaited further.
fn join_with_deadline<T: Send + 'static>(
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

impl Drop for BackgroundUploader {
    fn drop(&mut self) {
        // Signaling shutdown (dropping the sender, or setting the ring's
        // stop flag) lets the upload loop exit gracefully on its own.
        if let Some(inner) = self.inner.take() {
            let handle = match inner {
                UploaderInner::Unbounded(sender, handle) => {
                    drop(sender);
                    handle
                }
                UploaderInner::Ring(ring, handle) => {
                    ring.signal_stop();
                    handle
                }
            };
            debug!("Waiting for background uploader to shut down");
            if let Err(e) = handle.join() {
                error!("Background uploader thread panicked: {:?}", e);
            }
        }
    }
}

/// The two handle flavors a [`BackgroundUploadHandle`] can wrap,
/// mirroring [`UploaderInner`].
#[derive(Clone)]
enum HandleInner {
    Unbounded(WeakUnboundedSender<UploadData>),
    Ring(Weak<TelemetryRing>),
}

/// A handle to send data to the background uploader.
#[derive(Clone)]
pub struct BackgroundUploadHandle {
    inner: HandleInner,
}

impl BackgroundUploadHandle {
    /// Sends data to be uploaded in the background. Any 2xx response is
    /// treated as success; use [`Self::upload_with_validator`] if the
    /// destination's ingestion protocol can reject part of a request
    /// while still returning a 2xx status.
    pub fn upload(
        &self,
        url: &Url,
        body: impl Into<Vec<u8>>,
        timeout: Duration,
        content_type: Option<&'static str>,
    ) -> Result<(), Error> {
        self.upload_with_validator(url, body, timeout, content_type, None)
    }

    /// Same as [`Self::upload`], but with an optional response validator
    /// -- see `UploadData`'s `response_validator` field doc comment above
    /// for its contract.
    pub fn upload_with_validator(
        &self,
        url: &Url,
        body: impl Into<Vec<u8>>,
        timeout: Duration,
        content_type: Option<&'static str>,
        response_validator: Option<ResponseValidator>,
    ) -> Result<(), Error> {
        let data = UploadData {
            url: url.clone(),
            body: body.into(),
            timeout,
            content_type,
            response_validator,
        };
        match &self.inner {
            HandleInner::Unbounded(sender) => {
                if let Some(sender) = sender.upgrade() {
                    sender
                        .send(data)
                        .context("Failed to send data to background uploader")
                } else {
                    bail!("Background uploader has been shut down");
                }
            }
            HandleInner::Ring(ring) => {
                if let Some(ring) = ring.upgrade() {
                    ring.push(data)
                } else {
                    bail!("Background uploader has been shut down");
                }
            }
        }
    }

    /// Creates a new mock handle that does nothing.
    #[cfg(test)]
    pub fn new_mock() -> Self {
        let (tx, rx) = mpsc::unbounded_channel::<UploadData>();
        std::mem::drop(rx); // Drop the receiver to simulate a closed uploader
        Self {
            inner: HandleInner::Unbounded(tx.downgrade()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::{Duration, Instant};

    use mockito::{Matcher, Server};

    fn init_test_logging() {
        let _ = env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init();
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

    fn run_in_runtime(f: impl std::future::Future<Output = ()>) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(f);
    }

    #[test]
    /// Ensures `get_handle()` returns a weak sender that can no longer enqueue once the
    /// `BackgroundUploader` is dropped.
    fn test_handle_upload_errors_after_uploader_drop() {
        init_test_logging();

        let uploader = BackgroundUploader::new().unwrap();
        let handle = uploader.get_handle().unwrap();
        drop(uploader);

        let url = Url::parse("http://example.invalid/upload").unwrap();
        // After shutdown, the weak sender can't be upgraded so upload should error.
        let err = handle
            .upload(&url, b"hello".to_vec(), Duration::from_millis(50), None)
            .unwrap_err();
        assert!(
            err.to_string().contains("shut down"),
            "Unexpected error: {err:?}"
        );
    }

    #[test]
    /// Verifies the end-to-end happy path: `BackgroundUploader` accepts an upload request and
    /// eventually performs an HTTP POST with the provided body.
    fn test_background_uploader_sends_post_request() {
        init_test_logging();

        let uploader = BackgroundUploader::new().unwrap();
        let handle = uploader.get_handle().unwrap();

        let mut server = Server::new();
        let body = "hello-background-uploader";
        let mock = server
            .mock("POST", "/upload")
            .match_body(Matcher::Exact(body.to_string()))
            .with_status(200)
            .expect(1)
            .create();

        let url = Url::parse(&server.url()).unwrap().join("/upload").unwrap();
        handle
            .upload(&url, body.as_bytes().to_vec(), Duration::from_secs(2), None)
            .unwrap();

        // Drop uploader first to ensure the background thread finishes processing all queued
        // uploads before asserting. The Drop impl waits for the thread to join.
        drop(uploader);
        mock.assert();
    }

    #[test]
    /// Directly tests `upload_loop`: a queued message results in a single HTTP POST.
    fn test_upload_loop_sends_post_request() {
        init_test_logging();

        let mut server = Server::new();
        let body = "hello-upload-loop";
        let mock = server
            .mock("POST", "/upload")
            .match_body(Matcher::Exact(body.to_string()))
            .with_status(200)
            .expect(1)
            .create();

        run_in_runtime(async {
            let (sender, receiver) = mpsc::unbounded_channel::<UploadData>();
            let url = Url::parse(&server.url()).unwrap().join("/upload").unwrap();
            // Run the loop in a task so we can enqueue a message and then close the channel.
            let upload_task = tokio::spawn(async move {
                BackgroundUploader::upload_loop(receiver).await;
            });

            sender
                .send(UploadData {
                    url,
                    body: body.as_bytes().to_vec(),
                    timeout: Duration::from_secs(2),
                    content_type: None,
                    response_validator: None,
                })
                .unwrap();

            // Give the loop a moment to process the request before shutting down.
            tokio::time::sleep(Duration::from_millis(50)).await;
            drop(sender);
            let _ = upload_task.await;
        });
        mock.assert();
    }

    #[test]
    /// Directly tests `upload_loop` failure handling: once a request to an origin fails, future
    /// uploads to that same origin should be ignored.
    fn test_upload_loop_failed_host_is_ignored_for_future_uploads() {
        init_test_logging();

        // Use a single mockito server so both uploads share the same origin (scheme+host+port).
        // First upload: the server intentionally responds too slowly, causing a client timeout
        // (reqwest returns Err) which marks the origin as ignored.
        let mut server = Server::new();
        let slow_mock = server
            .mock("POST", "/slow")
            .with_status(200)
            .with_body_from_request(|_| {
                std::thread::sleep(Duration::from_millis(1000));
                b"slow-response".to_vec()
            })
            .expect(1)
            .create();

        let should_not_hit = server
            .mock("POST", "/upload")
            .with_status(200)
            .expect(0)
            .create();

        // Queue both requests upfront, then close the channel. The loop processes
        // messages sequentially, so the first request will timeout and mark the
        // origin as ignored before the second request is even considered.
        // This removes any timing dependency.
        let (sender, receiver) = mpsc::unbounded_channel::<UploadData>();

        // First request: a slow response + short timeout forces reqwest to return an error.
        // The timeout (100ms) must be long enough for the request to be sent to the server,
        // but short enough to expire before the mock's 1s response delay completes.
        sender
            .send(UploadData {
                url: Url::parse(&server.url()).unwrap().join("/slow").unwrap(),
                body: b"timeout-me".to_vec(),
                timeout: Duration::from_millis(100),
                content_type: None,
                response_validator: None,
            })
            .unwrap();

        // Second request: same origin; should be skipped after the first fails.
        sender
            .send(UploadData {
                url: Url::parse(&server.url()).unwrap().join("/upload").unwrap(),
                body: b"this-should-be-skipped".to_vec(),
                timeout: Duration::from_secs(2),
                content_type: None,
                response_validator: None,
            })
            .unwrap();

        // Close the channel before running the loop. The loop will process both
        // queued messages in order, then exit.
        drop(sender);

        run_in_runtime(async {
            BackgroundUploader::upload_loop(receiver).await;
        });

        slow_mock.assert();
        should_not_hit.assert();
    }

    #[test]
    /// Directly tests that a 3xx response (which `error_for_status()` alone would treat as
    /// success) is still handled as an upload failure: the origin gets ignored for later
    /// uploads, just like a 4xx/5xx response or a network-level error.
    fn test_upload_loop_redirect_status_is_treated_as_failure() {
        init_test_logging();

        let mut server = Server::new();
        let redirect_mock = server
            .mock("POST", "/redirect")
            .with_status(302)
            .expect(1)
            .create();

        let should_not_hit = server
            .mock("POST", "/upload")
            .with_status(200)
            .expect(0)
            .create();

        let (sender, receiver) = mpsc::unbounded_channel::<UploadData>();

        sender
            .send(UploadData {
                url: Url::parse(&server.url())
                    .unwrap()
                    .join("/redirect")
                    .unwrap(),
                body: b"redirect-me".to_vec(),
                timeout: Duration::from_secs(2),
                content_type: None,
                response_validator: None,
            })
            .unwrap();

        // Same origin; should be skipped after the first is treated as a failure.
        sender
            .send(UploadData {
                url: Url::parse(&server.url()).unwrap().join("/upload").unwrap(),
                body: b"this-should-be-skipped".to_vec(),
                timeout: Duration::from_secs(2),
                content_type: None,
                response_validator: None,
            })
            .unwrap();

        drop(sender);

        run_in_runtime(async {
            BackgroundUploader::upload_loop(receiver).await;
        });

        redirect_mock.assert();
        should_not_hit.assert();
    }

    #[test]
    /// The HTTP client must refuse to follow a redirect whose target is not
    /// `https`, even though the redirect response itself succeeds -- an
    /// HTTPS-only telemetry endpoint must never be silently downgraded to
    /// plaintext by a 307/308 redirect to an `http://` URL.
    fn test_upload_loop_rejects_redirect_to_non_https_url() {
        init_test_logging();

        let mut server = Server::new();
        let redirect_mock = server
            .mock("POST", "/redirect")
            .with_status(307)
            .with_header("location", "http://example.invalid/plaintext-upload")
            .expect(1)
            .create();

        let (sender, receiver) = mpsc::unbounded_channel::<UploadData>();

        sender
            .send(UploadData {
                url: Url::parse(&server.url())
                    .unwrap()
                    .join("/redirect")
                    .unwrap(),
                body: b"redirect-me".to_vec(),
                timeout: Duration::from_secs(2),
                content_type: None,
                response_validator: None,
            })
            .unwrap();

        drop(sender);

        run_in_runtime(async {
            BackgroundUploader::upload_loop(receiver).await;
        });

        // The redirect response was received, but the client must not have
        // actually sent a request to the plaintext `http://example.invalid`
        // target -- there is no mock for it, so a follow would panic/error
        // out with a connection failure rather than silently succeeding.
        redirect_mock.assert();
    }

    #[test]
    /// Directly tests `upload_loop` shutdown behavior: once the channel is closed, the loop
    /// should upload remaining items in the queue before exiting.
    fn test_upload_loop_shutdown_uploads_remaining_queue_items() {
        init_test_logging();

        // Deterministic shutdown behavior: if the channel is closed (sender dropped) after a
        // message has already been queued, `upload_loop` should still process that queued item.
        let mut server = Server::new();
        let queued_upload = server
            .mock("POST", "/queued")
            .with_status(200)
            .expect(1)
            .create();

        let (sender, receiver) = mpsc::unbounded_channel::<UploadData>();
        sender
            .send(UploadData {
                url: Url::parse(&server.url()).unwrap().join("/queued").unwrap(),
                body: b"queued".to_vec(),
                timeout: Duration::from_secs(1),
                content_type: None,
                response_validator: None,
            })
            .unwrap();
        // Close the sender before running the loop to simulate shutdown.
        drop(sender);

        run_in_runtime(async {
            BackgroundUploader::upload_loop(receiver).await;
        });
        queued_upload.assert();
    }

    #[test]
    /// Validates `get_handle()` weak/strong semantics:
    /// - handles can enqueue while the uploader is alive
    /// - cloned handles are still weak and fail once the uploader is dropped
    fn test_get_handle_weak_strong_semantics() {
        init_test_logging();

        let uploader = BackgroundUploader::new().unwrap();
        let handle = uploader
            .get_handle()
            .expect("get_handle should return Some when alive");
        let handle2 = handle.clone();

        let mut server = Server::new();
        let ok_mock = server
            .mock("POST", "/ok")
            .match_body(Matcher::Exact("hello".to_string()))
            .with_status(200)
            .expect(1)
            .create();

        let url = Url::parse(&server.url()).unwrap().join("/ok").unwrap();
        handle
            .upload(&url, b"hello".to_vec(), Duration::from_secs(2), None)
            .unwrap();

        // Drop the uploader to shut down the background thread. Both `handle`
        // and `handle2` should fail to upload after this point since they both
        // hold weak references. This also ensures that the background thread
        // has finished processing the queued upload before we assert.
        drop(uploader);
        ok_mock.assert();

        let after_drop = server
            .mock("POST", "/nope")
            .with_status(200)
            .expect(0)
            .create();

        let err = handle2
            .upload(
                &Url::parse(&server.url()).unwrap().join("/nope").unwrap(),
                b"nope".to_vec(),
                Duration::from_secs(1),
                None,
            )
            .unwrap_err();
        assert!(err.to_string().contains("shut down"));
        after_drop.assert();
    }

    #[test]
    fn test_shutdown_with_deadline_returns_promptly_with_empty_queue() {
        init_test_logging();

        let uploader = BackgroundUploader::new().unwrap();
        let start = Instant::now();
        uploader.shutdown_with_deadline(Duration::from_secs(5));
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "shutdown with nothing queued should be immediate"
        );
    }

    #[test]
    /// Deliberately not exercised via a real `BackgroundUploader` +
    /// network mock: a genuinely abandoned background thread would keep
    /// running past this test's own scope, in a process shared with every
    /// other test in the suite, risking exactly the kind of cross-test
    /// port/resource collisions a slow real HTTP mock invites under
    /// `cargo test`'s default parallelism. `join_with_deadline` is pure
    /// std-only plumbing (a thread + a timed channel receive), so testing
    /// it directly with a plain `thread::spawn` gives the same coverage
    /// without that risk.
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
    /// A `TelemetryRing` over capacity evicts the *oldest* not-yet-attempted
    /// entry, preserving FIFO order for the survivors, instead of blocking
    /// the caller or rejecting the new item.
    fn test_telemetry_ring_evicts_oldest_when_over_capacity() {
        let ring = TelemetryRing::new(2);
        let url = Url::parse("http://example.invalid/").unwrap();

        ring.push(mock_upload(url.clone(), "first")).unwrap();
        ring.push(mock_upload(url.clone(), "second")).unwrap();
        // Over capacity: "first" should be evicted, not "second".
        ring.push(mock_upload(url.clone(), "third")).unwrap();

        run_in_runtime(async {
            let first = ring.pop().await.unwrap();
            assert_eq!(first.body, b"second");
            let second = ring.pop().await.unwrap();
            assert_eq!(second.body, b"third");
        });
    }

    #[test]
    /// Before `signal_stop`, `pop()` waits for new items rather than
    /// returning `None` -- a ring with nothing queued yet is not the same
    /// as a shut-down one.
    fn test_telemetry_ring_pop_waits_for_push_before_stop() {
        let ring = TelemetryRing::new(4);
        let url = Url::parse("http://example.invalid/").unwrap();

        run_in_runtime(async {
            let ring_clone = ring.clone();
            let waiter = tokio::spawn(async move { ring_clone.pop().await });

            // Give the waiter a moment to actually start waiting before
            // pushing, so this also exercises the `Notify` wakeup path
            // rather than just the "already queued" fast path.
            tokio::time::sleep(Duration::from_millis(20)).await;
            ring.push(mock_upload(url, "hello")).unwrap();

            let item = tokio::time::timeout(Duration::from_secs(1), waiter)
                .await
                .expect("pop() should return once an item is pushed")
                .unwrap();
            assert_eq!(item.unwrap().body, b"hello");
        });
    }

    #[test]
    /// Once `signal_stop` is called, `pop()` drains whatever is left and
    /// then returns `None` -- it does not wait for further pushes.
    fn test_telemetry_ring_pop_returns_none_after_stop_and_drain() {
        let ring = TelemetryRing::new(4);
        let url = Url::parse("http://example.invalid/").unwrap();
        ring.push(mock_upload(url, "queued-before-stop")).unwrap();
        ring.signal_stop();

        run_in_runtime(async {
            let item = ring.pop().await;
            assert_eq!(item.unwrap().body, b"queued-before-stop");
            assert!(ring.pop().await.is_none());
        });
    }

    #[test]
    /// `ring_upload_loop`'s core shutdown behavior: while draining after
    /// `signal_stop`, the *first* failed upload stops the loop outright,
    /// abandoning whatever is left queued, rather than continuing through
    /// the whole backlog. Before `signal_stop`, a failure must not have
    /// this effect (covered by the existing `upload_loop` failure test,
    /// whose shared `attempt_upload` logic this loop also uses).
    fn test_ring_upload_loop_stops_draining_after_first_failure_once_stopped() {
        init_test_logging();

        let mut server = Server::new();
        // First item: a bad status the loop will treat as a failure.
        let failing = server
            .mock("POST", "/fail")
            .with_status(500)
            .expect(1)
            .create();
        // Second item: queued behind the failure, but should never be
        // attempted once the loop stops draining after that failure.
        let should_not_hit = server
            .mock("POST", "/skip")
            .with_status(200)
            .expect(0)
            .create();

        let ring = TelemetryRing::new(4);
        ring.push(mock_upload(
            Url::parse(&server.url()).unwrap().join("/fail").unwrap(),
            "will-fail",
        ))
        .unwrap();
        ring.push(mock_upload(
            Url::parse(&server.url()).unwrap().join("/skip").unwrap(),
            "should-be-abandoned",
        ))
        .unwrap();
        // Signal stop *before* the loop starts draining, so both queued
        // items are already present when draining begins.
        ring.signal_stop();

        run_in_runtime(async {
            BackgroundUploader::ring_upload_loop(ring).await;
        });

        failing.assert();
        should_not_hit.assert();
    }

    #[test]
    /// End-to-end: `BackgroundUploader::new_ring` behaves like a normal
    /// uploader for a single successful upload, exercising the ring path
    /// through the public API rather than `TelemetryRing`/`ring_upload_loop`
    /// directly.
    fn test_background_uploader_new_ring_sends_post_request() {
        init_test_logging();

        let mut server = Server::new();
        let body = "hello-ring-uploader";
        let mock = server
            .mock("POST", "/upload")
            .match_body(Matcher::Exact(body.to_string()))
            .with_status(200)
            .expect(1)
            .create();

        let uploader = BackgroundUploader::new_ring(4).unwrap();
        let handle = uploader.get_handle().unwrap();
        let url = Url::parse(&server.url()).unwrap().join("/upload").unwrap();
        handle
            .upload(&url, body.as_bytes().to_vec(), Duration::from_secs(2), None)
            .unwrap();

        drop(uploader);
        mock.assert();
    }
}
