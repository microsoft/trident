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

            let result = HTTP_ASYNC_CLIENT
                .post(upload.url.clone())
                .timeout(upload.timeout)
                .body(upload.body)
                .send()
                .await;

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

            // Note: we don't particularly care much for the status code since
            // this is just a generic implementation.
        }

        debug!("Background uploader loop has exited");
    }
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
    ) -> Result<(), Error> {
        if let Some(sender) = self.sender.upgrade() {
            sender
                .send(UploadData {
                    url: url.clone(),
                    body: body.into(),
                    timeout,
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

    use std::time::Duration;

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
            .upload(&url, b"hello".to_vec(), Duration::from_millis(50))
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
            .upload(&url, body.as_bytes().to_vec(), Duration::from_secs(2))
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
                std::thread::sleep(Duration::from_millis(200));
                b"ok".to_vec()
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
        sender
            .send(UploadData {
                url: Url::parse(&server.url()).unwrap().join("/slow").unwrap(),
                body: b"timeout-me".to_vec(),
                timeout: Duration::from_millis(10),
            })
            .unwrap();

        // Second request: same origin; should be skipped after the first fails.
        sender
            .send(UploadData {
                url: Url::parse(&server.url()).unwrap().join("/upload").unwrap(),
                body: b"this-should-be-skipped".to_vec(),
                timeout: Duration::from_secs(2),
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
            .upload(&url, b"hello".to_vec(), Duration::from_secs(2))
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
            )
            .unwrap_err();
        assert!(err.to_string().contains("shut down"));
        after_drop.assert();
    }
}
