use std::time::Duration;

use tokio::time;
use tonic::Streaming;

use trident_proto::v1::ServicingResponse as ProtoServicingResponse;

use crate::{
    protocol::{self, Operation},
    ClientError, Completion, ServicingEvent,
};

/// Accepted servicing request with a domain-event response stream.
#[must_use = "receiving a ServicingOperation only acknowledges the RPC; call wait, wait_with, or next_event to observe its outcome"]
pub struct ServicingOperation {
    operation: Operation,
    stream: Streaming<ProtoServicingResponse>,
    completed: bool,
    completion_timeout: Option<Duration>,
}

impl ServicingOperation {
    pub(crate) fn new(operation: Operation, stream: Streaming<ProtoServicingResponse>) -> Self {
        Self {
            operation,
            stream,
            completed: false,
            completion_timeout: None,
        }
    }

    /// Applies a timeout to [`wait`](Self::wait) and
    /// [`wait_with`](Self::wait_with). Timing starts when either method is
    /// called, after the request has been accepted.
    pub fn with_completion_timeout(mut self, timeout: Duration) -> Self {
        self.completion_timeout = Some(timeout);
        self
    }

    /// Returns the next domain event.
    pub async fn next_event(&mut self) -> Result<Option<ServicingEvent>, ClientError> {
        if self.completed {
            return Ok(None);
        }

        let response = self
            .stream
            .message()
            .await
            .map_err(|status| ClientError::stream_failure(self.operation, status))?
            .ok_or_else(|| ClientError::missing_completion(self.operation))?;
        self.completed = protocol::servicing_response_completes(&response);
        let event = protocol::decode_servicing_response(self.operation, response)?;
        Ok(Some(event))
    }

    /// Consumes events until successful completion.
    pub async fn wait(self) -> Result<Completion, ClientError> {
        self.wait_with(|_| {}).await
    }

    /// Consumes events, invoking `on_event` for each one.
    pub async fn wait_with<F>(mut self, mut on_event: F) -> Result<Completion, ClientError>
    where
        F: FnMut(&ServicingEvent),
    {
        let operation = self.operation;
        let timeout = self.completion_timeout;
        let wait = async {
            loop {
                let event = self.next_event().await?;
                let Some(event) = event else {
                    return Err(ClientError::missing_completion(operation));
                };
                on_event(&event);
                match event {
                    ServicingEvent::Completed(Ok(completion)) => return Ok(completion),
                    ServicingEvent::Completed(Err(remote)) => {
                        return Err(ClientError::remote_failure(operation, remote));
                    }
                    ServicingEvent::Started { .. } | ServicingEvent::Log(_) => {}
                }
            }
        };

        match timeout {
            Some(timeout) => time::timeout(timeout, wait)
                .await
                .map_err(|_| ClientError::completion_timeout(operation, timeout))?,
            None => wait.await,
        }
    }
}
