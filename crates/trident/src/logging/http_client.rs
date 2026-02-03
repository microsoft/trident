use std::sync::LazyLock;

use anyhow::{Context, Error};
use reqwest::{blocking::Client as BlockingClient, Client as AsyncClient};
use tokio::runtime::Handle;

pub(super) static HTTP_CLIENT: LazyLock<HttpClient> = LazyLock::new(HttpClient::new);

/// A temporary wrapper around reqwest's blocking and async clients to
/// dynamically pick the right one depending on the runtime context.
pub(super) struct HttpClient {
    blocking: BlockingClient,
    async_client: AsyncClient,
}

impl HttpClient {
    pub fn new() -> Self {
        Self {
            blocking: BlockingClient::new(),
            async_client: AsyncClient::new(),
        }
    }

    pub fn post(&self, url: &str, body: impl Into<String>) -> Result<(), Error> {
        let body = body.into();
        if let Ok(handle) = Handle::try_current() {
            // If we're running within a Tokio runtime, we may be on a runtime worker thread.
            // Using `block_in_place` avoids panicking when synchronously waiting for an async
            // request.
            let client = self.async_client.clone();
            let body = body.clone();
            tokio::task::block_in_place(|| {
                handle.block_on(async move { client.post(url).body(body).send().await })
            })
            .context("Failed to send log over HTTP (async)")?;
        } else {
            self.blocking
                .post(url)
                .body(body)
                .send()
                .context("Failed to send log synchronously")?;
        }

        Ok(())
    }
}
