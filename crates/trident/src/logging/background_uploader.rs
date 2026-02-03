use std::sync::LazyLock;

use anyhow::{bail, Context, Error};
use reqwest::Client;
use tokio::sync::{
    mpsc::{self, UnboundedReceiver, UnboundedSender, WeakUnboundedSender},
    oneshot,
};
use url::Url;

static HTTP_ASYNC_CLIENT: LazyLock<Client> = LazyLock::new(Client::new);

pub struct UploadData {
    pub url: Url,
    pub body: Vec<u8>,
}

/// A background uploader that sends log data to a remote server asynchronously.
pub struct BackgroundUploader {
    sender: UnboundedSender<UploadData>,
}

impl BackgroundUploader {
    /// Creates a new background uploader.
    pub fn new() -> Result<Self, Error> {
        let (sender, receiver) = mpsc::unbounded_channel();
        Self::start_upload_task(receiver)?;
        Ok(Self { sender })
    }

    /// Gets a handle to send data to the uploader.
    pub fn get_handle(&self) -> BackgroundUploadHandle {
        BackgroundUploadHandle {
            sender: self.sender.downgrade(),
        }
    }

    /// Gracefully shuts down the uploader, waiting for all pending uploads to finish.
    pub fn finish(self) {
        drop(self.sender);
    }

    /// Starts a new thread with a Tokio runtime to handle uploads.
    fn start_upload_task(receiver: UnboundedReceiver<UploadData>) -> Result<(), Error> {
        let (ready_tx, ready_rx) = oneshot::channel::<bool>();
        std::thread::spawn(move || {
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
            Ok(true) => Ok(()),
            Ok(false) => bail!("Failed to create Tokio runtime for background uploader"),
            Err(e) => bail!("Background uploader thread terminated unexpectedly: {e}"),
        }
    }

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
    }
}

/// A handle to send data to the background uploader.
pub struct BackgroundUploadHandle {
    sender: WeakUnboundedSender<UploadData>,
}

impl BackgroundUploadHandle {
    /// Sends data to be uploaded in the background.
    pub fn upload(&self, url: Url, body: impl Into<Vec<u8>>) -> Result<(), Error> {
        if let Some(sender) = self.sender.upgrade() {
            sender
                .send(UploadData {
                    url,
                    body: body.into(),
                })
                .context("Failed to send data to background uploader")
        } else {
            bail!("Background uploader has been shut down");
        }
    }
}
