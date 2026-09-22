use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, Weak,
    },
    thread::JoinHandle,
    time::Duration,
};

use anyhow::{bail, Error};
use log::{debug, error};
use tokio::sync::Notify;
use url::Url;

use super::upload_core::{
    self, attempt_upload, join_with_deadline, AttemptOutcome, OriginCooldowns, ResponseValidator,
    UploadData,
};

/// A bounded, non-blocking FIFO queue of pending uploads: pushing past
/// `capacity` evicts the oldest not-yet-attempted entry instead of
/// blocking the producer or growing without bound. Used for telemetry --
/// see [`TelemetryUploader::new`] -- where producers (a
/// `tracing_subscriber::Layer` callback) must never block or fail on
/// enqueue, ruling out a backpressured bounded channel (whose `send()`
/// either awaits or rejects when full, neither of which is acceptable
/// here), and where a genuinely *failing* endpoint is already
/// self-limiting via `attempt_upload`'s per-origin cooldown, but a merely
/// *slow* one is not: nothing would otherwise cap how large the backlog
/// (or its memory) could grow while such an endpoint kept the loop busy.
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
    /// unbounded uploader's (logged, backed off, keep going).
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

/// A background uploader for best-effort Application Insights telemetry,
/// backed by a bounded [`TelemetryRing`] instead of the unbounded queue
/// [`super::background_uploader::BackgroundUploader`] uses for real log
/// forwarding -- see `TelemetryRing`'s doc comment for why telemetry needs a
/// different queueing strategy. Shares its actual POST/backoff logic with
/// that uploader via [`super::upload_core`].
///
/// When dropped it will finish any pending uploads and shut down the
/// background thread; use [`Self::shutdown_with_deadline`] instead if that
/// must be bounded (see its doc comment).
pub struct TelemetryUploader {
    inner: Option<(Arc<TelemetryRing>, JoinHandle<()>)>,
}

impl TelemetryUploader {
    /// Creates a new telemetry uploader with a pending-upload queue bounded
    /// to at most `capacity` entries. Producers must never block or fail on
    /// enqueue, and low telemetry volume plus the daemon's short-lived-
    /// between-sparse-requests lifecycle make actual unbounded growth
    /// unlikely in practice, but capping it is cheap insurance against a
    /// slow-but-succeeding endpoint on a busier or longer-lived process
    /// than typically expected.
    pub fn new(capacity: usize) -> Result<Self, Error> {
        let ring = TelemetryRing::new(capacity);
        let handle = upload_core::spawn_uploader_thread(Self::ring_upload_loop(ring.clone()))?;
        Ok(Self {
            inner: Some((ring, handle)),
        })
    }

    /// Gets a handle to send data to the uploader. Returns `None` if the uploader has been shut down.
    pub fn get_handle(&self) -> Option<TelemetryUploadHandle> {
        let (ring, _) = self.inner.as_ref()?;
        Some(TelemetryUploadHandle {
            ring: Arc::downgrade(ring),
        })
    }

    /// The main upload loop that processes incoming upload requests from
    /// the ring. Behaves identically to
    /// [`super::background_uploader::BackgroundUploader::upload_loop`]
    /// during normal operation (a failed upload is logged, backed off via
    /// `origin_cooldowns`, and the loop keeps going) -- it diverges only
    /// once shutdown has been requested (`ring.pop()` returning items
    /// after `signal_stop` was called): from that point, the *first*
    /// failed upload stops the loop outright, abandoning whatever is left
    /// queued, rather than continuing to drain it.
    ///
    /// This is a deliberate choice over draining the entire backlog
    /// unconditionally: `shutdown_with_deadline`'s external deadline
    /// already hard-caps how long anything waits on this loop, so a large
    /// ring full of slow-but-succeeding uploads is not a correctness risk
    /// either way -- but stopping on first failure avoids spending that
    /// bounded shutdown window on retries to an endpoint that's *also* now
    /// failing, in favor of exiting promptly.
    async fn ring_upload_loop(ring: Arc<TelemetryRing>) {
        let mut origin_cooldowns = OriginCooldowns::new();

        while let Some(upload) = ring.pop().await {
            let draining = ring.stop_requested.load(Ordering::Acquire);
            let outcome = attempt_upload(&mut origin_cooldowns, upload).await;
            if draining && matches!(outcome, AttemptOutcome::Attempted { succeeded: false }) {
                debug!(
                    "Telemetry uploader stopping drain after a failed upload during shutdown"
                );
                break;
            }
        }

        debug!("Telemetry uploader loop has exited");
    }

    /// Signals the uploader to shut down, waiting up to `deadline` for its
    /// background thread to drain whatever is already queued and exit.
    ///
    /// `Drop`'s own shutdown (used when this isn't called explicitly) waits
    /// unboundedly. Telemetry callers should call this explicitly instead,
    /// since a slow-but-successful endpoint could otherwise stall process
    /// exit for as long as it takes to drain every queued event -- telemetry
    /// must never meaningfully delay Trident's actual work, including at
    /// shutdown.
    ///
    /// If `deadline` elapses first, the background thread is abandoned
    /// (its remaining queued requests may still complete before the
    /// process actually exits, but this call returns without waiting
    /// further for them).
    pub fn shutdown_with_deadline(mut self, deadline: Duration) {
        let Some((ring, handle)) = self.inner.take() else {
            return;
        };
        ring.signal_stop();

        match join_with_deadline(handle, deadline) {
            Ok(Ok(())) => debug!("Telemetry uploader shut down"),
            Ok(Err(e)) => error!("Telemetry uploader thread panicked: {:?}", e),
            Err(_) => {
                debug!("Telemetry uploader did not shut down within {deadline:?}; abandoning it")
            }
        }
    }
}

impl Drop for TelemetryUploader {
    fn drop(&mut self) {
        // Signaling shutdown lets the upload loop exit gracefully on its own.
        if let Some((ring, handle)) = self.inner.take() {
            ring.signal_stop();
            debug!("Waiting for telemetry uploader to shut down");
            if let Err(e) = handle.join() {
                error!("Telemetry uploader thread panicked: {:?}", e);
            }
        }
    }
}

/// A handle to send data to the telemetry uploader.
#[derive(Clone)]
pub struct TelemetryUploadHandle {
    ring: Weak<TelemetryRing>,
}

impl TelemetryUploadHandle {
    /// Sends data to be uploaded in the background. Any 2xx response is
    /// treated as success; use [`Self::upload_with_validator`] if the
    /// destination's ingestion protocol can reject part of a request
    /// while still returning a 2xx status.
    #[cfg(test)]
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
    /// -- see `upload_core::UploadData`'s `response_validator` field doc
    /// comment for its contract.
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
        if let Some(ring) = self.ring.upgrade() {
            ring.push(data)
        } else {
            bail!("Background uploader has been shut down");
        }
    }

    /// Creates a new mock handle that does nothing.
    #[cfg(test)]
    pub fn new_mock() -> Self {
        let ring = TelemetryRing::new(0);
        ring.signal_stop();
        Self {
            ring: Arc::downgrade(&ring),
        }
        // `ring` itself is dropped here (only a `Weak` is kept), so the
        // handle behaves like one whose uploader has already shut down --
        // matching `BackgroundUploadHandle::new_mock`'s "does nothing"
        // contract.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use mockito::Matcher;

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
    /// the whole backlog.
    fn test_ring_upload_loop_stops_draining_after_first_failure_once_stopped() {
        init_test_logging();

        let mut server = mockito::Server::new();
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
            TelemetryUploader::ring_upload_loop(ring).await;
        });

        failing.assert();
        should_not_hit.assert();
    }

    #[test]
    /// End-to-end: `TelemetryUploader::new` behaves like a normal uploader
    /// for a single successful upload, exercising the ring path through the
    /// public API rather than `TelemetryRing`/`ring_upload_loop` directly.
    fn test_telemetry_uploader_new_sends_post_request() {
        init_test_logging();

        let mut server = mockito::Server::new();
        let body = "hello-telemetry-uploader";
        let mock = server
            .mock("POST", "/upload")
            .match_body(Matcher::Exact(body.to_string()))
            .with_status(200)
            .expect(1)
            .create();

        let uploader = TelemetryUploader::new(4).unwrap();
        let handle = uploader.get_handle().unwrap();
        let url = Url::parse(&server.url()).unwrap().join("/upload").unwrap();
        handle
            .upload(&url, body.as_bytes().to_vec(), Duration::from_secs(2), None)
            .unwrap();

        drop(uploader);
        mock.assert();
    }
}
