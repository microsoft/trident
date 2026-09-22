use std::{
    thread::JoinHandle,
    time::Duration,
};

use anyhow::{bail, Context, Error};
use log::{debug, error};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender, WeakUnboundedSender};
use url::Url;

use super::upload_core::{
    self, attempt_upload, join_with_deadline, OriginCooldowns, ResponseValidator, UploadData,
};

// Re-exported so callers filtering log forwarding by this uploader's own
// module don't need to know that the actual POST/backoff logic (and its
// logging) lives in `upload_core`.
pub(super) use super::upload_core::BACKGROUND_LOG_MODULE;

/// A background uploader that sends log data to a remote server
/// asynchronously, via an unbounded pending-upload queue -- losing queued
/// data is not acceptable, so items are never dropped under backpressure.
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
        upload_core::spawn_uploader_thread(Self::upload_loop(receiver))
    }

    /// The main upload loop that processes incoming upload requests. Runs
    /// until the channel is closed (all senders dropped) and fully
    /// drained.
    async fn upload_loop(mut receiver: UnboundedReceiver<UploadData>) {
        let mut origin_cooldowns = OriginCooldowns::new();

        while let Some(upload) = receiver.recv().await {
            attempt_upload(&mut origin_cooldowns, upload).await;
        }

        debug!("Background uploader loop has exited");
    }

    /// Signals the uploader to shut down, waiting up to `deadline` for its
    /// background thread to drain whatever is already queued and exit.
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

impl Drop for BackgroundUploader {
    fn drop(&mut self) {
        // Dropping the sender lets the upload loop exit gracefully on its
        // own once it has drained whatever is already queued.
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
        if let Some(sender) = self.sender.upgrade() {
            sender
                .send(data)
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

    use std::time::Instant;

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

        let mut server = mockito::Server::new();
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

        let mut server = mockito::Server::new();
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
    /// Directly tests `upload_loop` shutdown behavior: once the channel is closed, the loop
    /// should upload remaining items in the queue before exiting.
    fn test_upload_loop_shutdown_uploads_remaining_queue_items() {
        init_test_logging();

        // Deterministic shutdown behavior: if the channel is closed (sender dropped) after a
        // message has already been queued, `upload_loop` should still process that queued item.
        let mut server = mockito::Server::new();
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

        let mut server = mockito::Server::new();
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
}
