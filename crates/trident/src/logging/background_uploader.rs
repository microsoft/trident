use std::{sync::LazyLock, thread::JoinHandle};

use anyhow::{bail, Context, Error};
use log::debug;
use reqwest::Client;
use tokio::sync::{
    mpsc::{self, UnboundedReceiver, UnboundedSender, WeakUnboundedSender},
    oneshot,
};
use url::Url;

static HTTP_ASYNC_CLIENT: LazyLock<Client> = LazyLock::new(Client::new);

/// Data to be uploaded by the background uploader.
struct UploadData {
    pub url: Url,
    pub body: Vec<u8>,
}

/// A background uploader that sends log data to a remote server asynchronously.
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
        let handle = std::thread::spawn(move || {
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
        });

        // Wait for the runtime to be ready
        match ready_rx.blocking_recv() {
            Ok(true) => Ok(handle),
            Ok(false) => bail!("Failed to create Tokio runtime for background uploader"),
            Err(e) => bail!("Background uploader thread terminated unexpectedly: {e}"),
        }
    }

    /// The main upload loop that processes incoming upload requests.
    async fn upload_loop(mut receiver: UnboundedReceiver<UploadData>) {
        while let Some(upload) = receiver.recv().await {
            let result = HTTP_ASYNC_CLIENT
                .post(upload.url)
                .body(upload.body)
                .send()
                .await;

            if let Err(e) = result {
                eprintln!("Background upload failed: {e}");
            }
        }

        log::debug!("Background uploader loop has exited");
    }
}

impl Drop for BackgroundUploader {
    fn drop(&mut self) {
        // When the sender is dropped, the upload loop will exit gracefully
        if let Some((sender, handle)) = self.inner.take() {
            drop(sender);
            debug!("Waiting for background uploader to shut down");
            let _ = handle.join();
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
    pub fn upload(&self, url: &Url, body: impl Into<Vec<u8>>) -> Result<(), Error> {
        if let Some(sender) = self.sender.upgrade() {
            sender
                .send(UploadData {
                    url: url.clone(),
                    body: body.into(),
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
