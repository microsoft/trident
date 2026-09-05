use std::{
    collections::HashSet,
    sync::LazyLock,
    thread::{Builder, JoinHandle},
    time::Duration,
};

use anyhow::{bail, Context, Error};
use log::{debug, error};
use reqwest::Client;
use tokio::sync::{
    mpsc::{self, UnboundedReceiver, UnboundedSender, WeakUnboundedSender},
    oneshot,
};
use url::{Origin, Url};

/// A static HTTP client for background uploads.
static HTTP_ASYNC_CLIENT: LazyLock<Client> = LazyLock::new(Client::new);

/// The module path of the background uploader. Can be used for filtering logs.
pub(super) const BACKGROUND_LOG_MODULE: &str = module_path!();

/// Data to be uploaded by the background uploader.
struct UploadData {
    url: Url,
    body: Vec<u8>,
    timeout: Duration,
    /// Optional `Content-Type` header value to attach to the request.
    content_type: Option<&'static str>,
}

/// A background uploader that sends log data to a remote server asynchronously.
///
/// When dropped it will finish any pending uploads and shut down the background
/// thread.
pub struct BackgroundUploader {
    inner: Option<(UnboundedSender<UploadData>, JoinHandle<()>)>,
}

impl BackgroundUploader {
    /// Creates a new background uploader.
    pub fn new() -> Result<Self, Error> {
        let (sender, receiver) = mpsc::unbounded_channel();
        let handle = Self::start_upload_task(receiver)?;
        Ok(Self {
            inner: Some((sender, handle)),
        })
    }

    /// Gets a handle to send data to the uploader. Returns `None` if the uploader has been shut down.
    pub fn get_handle(&self) -> Option<BackgroundUploadHandle> {
        Some(BackgroundUploadHandle {
            sender: self.inner.as_ref().map(|(sender, _)| sender)?.downgrade(),
        })
    }

    /// Starts a new thread with a Tokio runtime to handle uploads.
    fn start_upload_task(receiver: UnboundedReceiver<UploadData>) -> Result<JoinHandle<()>, Error> {
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

                runtime.block_on(async move {
                    Self::upload_loop(receiver).await;
                });
            })
            .context("Failed to create background-uploader thread.")?;

        // Wait for the runtime to be ready
        match ready_rx.blocking_recv() {
            Ok(true) => Ok(handle),
            Ok(false) => bail!("Failed to create Tokio runtime for background uploader"),
            Err(e) => bail!("Background uploader thread terminated unexpectedly: {e}"),
        }
    }

    /// The main upload loop that processes incoming upload requests.
    async fn upload_loop(mut receiver: UnboundedReceiver<UploadData>) {
        let mut ignored_servers = HashSet::new();

        while let Some(upload) = receiver.recv().await {
            if ignored_servers.contains(&upload.url.origin()) {
                continue;
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
            // silently treating any response as success.
            let result = request
                .send()
                .await
                .and_then(|response| response.error_for_status());

            if let Err(e) = result {
                error!("Background upload failed: {e}");
                ignored_servers.insert(upload.url.origin());
                error!(
                    "Ignoring future uploads to server: {}",
                    match upload.url.origin() {
                        Origin::Tuple(scheme, host, port) =>
                            format!("{}://{}:{}", scheme, host, port),
                        Origin::Opaque(_) => "[opaque origin]".to_string(),
                    }
                );
            }
        }

        debug!("Background uploader loop has exited");
    }
}

impl BackgroundUploader {
    /// Signals the uploader to shut down, waiting up to `deadline` for its
    /// background thread to drain whatever is already queued and exit.
    ///
    /// `Drop`'s own shutdown (used when this isn't called explicitly) waits
    /// unboundedly: `ignored_servers` (see `start_upload_task`) bounds the
    /// wait for an origin that outright *fails*, since only the first
    /// request to it is ever actually attempted, but a slow-but-successful
    /// endpoint is not bounded that way -- every queued request still gets
    /// its own attempt, each up to that request's own timeout, so draining
    /// a large backlog could still take a while. Callers for whom that
    /// matters (telemetry in particular: "must never meaningfully delay
    /// Trident's actual work" is a stated design goal here) should call
    /// this explicitly instead of just letting the value drop.
    ///
    /// If `deadline` elapses first, the background thread is abandoned
    /// (its remaining queued requests may still complete before the
    /// process actually exits, but this call returns without waiting
    /// further for them).
    pub fn shutdown_with_deadline(mut self, deadline: Duration) {
        let Some((sender, handle)) = self.inner.take() else {
            return;
        };
        drop(sender);

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
        // When the sender is dropped, the upload loop will exit gracefully
        if let Some((sender, handle)) = self.inner.take() {
            drop(sender);
            debug!("Waiting for background uploader to shut down");
            if let Err(e) = handle.join() {
                error!("Background uploader thread panicked: {:?}", e);
            }
        }
    }
}

/// A handle to send data to the background uploader.
#[derive(Clone)]
pub struct BackgroundUploadHandle {
    sender: WeakUnboundedSender<UploadData>,
}

impl BackgroundUploadHandle {
    /// Sends data to be uploaded in the background.
    pub fn upload(
        &self,
        url: &Url,
        body: impl Into<Vec<u8>>,
        timeout: Duration,
        content_type: Option<&'static str>,
    ) -> Result<(), Error> {
        if let Some(sender) = self.sender.upgrade() {
            sender
                .send(UploadData {
                    url: url.clone(),
                    body: body.into(),
                    timeout,
                    content_type,
                })
                .context("Failed to send data to background uploader")
        } else {
            bail!("Background uploader has been shut down");
        }
    }

    /// Creates a new mock handle that does nothing.
    #[cfg(test)]
    pub fn new_mock() -> Self {
        let (tx, rx) = mpsc::unbounded_channel::<UploadData>();
        std::mem::drop(rx); // Drop the receiver to simulate a closed uploader
        Self {
            sender: tx.downgrade(),
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
            })
            .unwrap();

        // Second request: same origin; should be skipped after the first fails.
        sender
            .send(UploadData {
                url: Url::parse(&server.url()).unwrap().join("/upload").unwrap(),
                body: b"this-should-be-skipped".to_vec(),
                timeout: Duration::from_secs(2),
                content_type: None,
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
}
