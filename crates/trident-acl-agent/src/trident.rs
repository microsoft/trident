//! gRPC helpers for talking to `tridentd`.
//!
//! The label protocol needs stage/finalize/commit plus a startup servicing-state
//! query (§4–§5). Harpoon uses the preview `StatusService::GetServicingState`
//! when available; if the daemon doesn't expose it yet, the orchestrator logs a
//! warning and falls back to label-based progress only.

use std::time::Duration;

use anyhow::anyhow;
use futures::StreamExt;
use tonic::{transport::Endpoint, Request, Streaming};
use trident_proto::{
    v1::{
        commit_service_client::CommitServiceClient, servicing_response::Response as ResponseBody,
        update_service_client::UpdateServiceClient, CommitRequest, FinalizeUpdateRequest,
        HostConfiguration, LogLevel, RebootHandling, RebootManagement, RebootStatus, ServicingKind,
        ServicingResponse, StageUpdateRequest, StatusCode, TridentErrorKind, UpdateRequest,
    },
    v1preview::{
        status_service_client::StatusServiceClient, GetLastErrorRequest, GetServicingStateRequest,
        ServicingState as PreviewServicingState,
    },
};
use url::Url;

#[derive(Debug, Clone)]
pub struct CompletedResponse {
    pub reboot_status: RebootStatus,
    pub servicing_kind: Option<ServicingKind>,
}

#[derive(Debug, Clone)]
pub struct RemoteError {
    pub kind: Option<TridentErrorKind>,
    pub subkind: String,
    pub message: String,
    pub error_message: String,
}

#[derive(Debug, thiserror::Error)]
pub enum TridentClientError {
    #[error("failed to connect to trident socket {socket}: {source}")]
    Connect {
        socket: String,
        #[source]
        source: tonic::transport::Error,
    },
    #[error("failed to start trident request {operation}: {source}")]
    Request {
        operation: &'static str,
        #[source]
        source: tonic::Status,
    },
    #[error("trident stream for {operation} ended before a Completed message")]
    MissingCompletion { operation: &'static str },
    #[error("trident stream for {operation} failed: {source}")]
    Stream {
        operation: &'static str,
        #[source]
        source: anyhow::Error,
    },
    #[error("trident reported {operation} failure: {details:?}")]
    Remote {
        operation: &'static str,
        details: RemoteError,
    },
    #[error("{operation} timed out after {timeout:?}")]
    Timeout {
        operation: &'static str,
        timeout: Duration,
    },
}

impl TridentClientError {
    pub fn remote(&self) -> Option<&RemoteError> {
        match self {
            Self::Remote { details, .. } => Some(details),
            _ => None,
        }
    }
}

pub struct TridentClient {
    update_client: UpdateServiceClient<tonic::transport::Channel>,
    commit_client: CommitServiceClient<tonic::transport::Channel>,
    status_client: StatusServiceClient<tonic::transport::Channel>,
}

impl TridentClient {
    pub async fn connect(socket: &str) -> Result<Self, TridentClientError> {
        let endpoint =
            Endpoint::new(socket.to_string()).map_err(|source| TridentClientError::Connect {
                socket: socket.to_string(),
                source,
            })?;
        let channel = endpoint
            .connect()
            .await
            .map_err(|source| TridentClientError::Connect {
                socket: socket.to_string(),
                source,
            })?;

        Ok(Self {
            update_client: UpdateServiceClient::new(channel.clone()),
            commit_client: CommitServiceClient::new(channel.clone()),
            status_client: StatusServiceClient::new(channel),
        })
    }

    pub async fn update(
        &mut self,
        url: &Url,
        hash: Option<&str>,
        timeout: Duration,
    ) -> Result<CompletedResponse, TridentClientError> {
        let response = self
            .update_client
            .update(Request::new(UpdateRequest {
                stage: Some(StageUpdateRequest {
                    config: Some(host_configuration_from_image(url, hash)),
                }),
                finalize: Some(FinalizeUpdateRequest {
                    reboot: Some(RebootManagement {
                        handling: RebootHandling::CallerHandlesReboot.into(),
                    }),
                }),
            }))
            .await
            .map_err(|source| TridentClientError::Request {
                operation: "update",
                source,
            })?
            .into_inner();

        run_with_timeout(
            "update",
            timeout,
            consume_servicing_stream("update", response),
        )
        .await
    }

    pub async fn update_stage(
        &mut self,
        url: &Url,
        hash: Option<&str>,
        timeout: Duration,
    ) -> Result<CompletedResponse, TridentClientError> {
        let response = self
            .update_client
            .update_stage(Request::new(StageUpdateRequest {
                config: Some(host_configuration_from_image(url, hash)),
            }))
            .await
            .map_err(|source| TridentClientError::Request {
                operation: "update_stage",
                source,
            })?
            .into_inner();

        run_with_timeout(
            "update_stage",
            timeout,
            consume_servicing_stream("update_stage", response),
        )
        .await
    }

    pub async fn update_finalize(
        &mut self,
        timeout: Duration,
    ) -> Result<CompletedResponse, TridentClientError> {
        let response = self
            .update_client
            .update_finalize(Request::new(FinalizeUpdateRequest {
                reboot: Some(RebootManagement {
                    handling: RebootHandling::CallerHandlesReboot.into(),
                }),
            }))
            .await
            .map_err(|source| TridentClientError::Request {
                operation: "update_finalize",
                source,
            })?
            .into_inner();

        run_with_timeout(
            "update_finalize",
            timeout,
            consume_servicing_stream("update_finalize", response),
        )
        .await
    }

    pub async fn commit(
        &mut self,
        timeout: Duration,
    ) -> Result<CompletedResponse, TridentClientError> {
        let response = self
            .commit_client
            .commit(Request::new(CommitRequest {
                reboot: Some(RebootManagement {
                    handling: RebootHandling::TridentHandlesReboot.into(),
                }),
            }))
            .await
            .map_err(|source| TridentClientError::Request {
                operation: "commit",
                source,
            })?
            .into_inner();

        run_with_timeout(
            "commit",
            timeout,
            consume_servicing_stream("commit", response),
        )
        .await
    }

    pub async fn get_servicing_state(
        &mut self,
    ) -> Result<PreviewServicingState, TridentClientError> {
        let response = self
            .status_client
            .get_servicing_state(Request::new(GetServicingStateRequest {}))
            .await
            .map_err(|source| TridentClientError::Request {
                operation: "get_servicing_state",
                source,
            })?;

        Ok(response.into_inner().state())
    }

    /// Returns the last error tridentd recorded for the most recent servicing
    /// operation, if any. Used to distinguish a genuine post-reboot resume
    /// from a bare process restart on the pre-update boot: per operator
    /// guidance, `UpdateAbFinalized` with no last error means the agent
    /// restarted without the machine actually rebooting (finalize completed,
    /// but the reboot never took effect), while `UpdateAbFinalized` *with* a
    /// last error - or `Provisioned` in either case - indicates a real reboot
    /// occurred.
    pub async fn get_last_error(
        &mut self,
    ) -> Result<Option<RemoteError>, TridentClientError> {
        let response = self
            .status_client
            .get_last_error(Request::new(GetLastErrorRequest {}))
            .await
            .map_err(|source| TridentClientError::Request {
                operation: "get_last_error",
                source,
            })?;

        Ok(response.into_inner().error.map(|error| RemoteError {
            kind: TridentErrorKind::try_from(error.kind).ok(),
            subkind: error.subkind,
            message: error.message,
            error_message: error.error_message,
        }))
    }
}

pub fn host_configuration_from_image(url: &Url, hash: Option<&str>) -> HostConfiguration {
    HostConfiguration {
        config: match hash {
            Some(hash) => format!("image:\n  url: {url}\n  sha384: {hash}"),
            None => format!("image:\n  url: {url}\n  sha384: ignored"),
        },
    }
}

async fn run_with_timeout<T>(
    operation: &'static str,
    timeout: Duration,
    future: impl std::future::Future<Output = Result<T, TridentClientError>>,
) -> Result<T, TridentClientError> {
    tokio::time::timeout(timeout, future)
        .await
        .map_err(|_| TridentClientError::Timeout { operation, timeout })?
}

async fn consume_servicing_stream(
    operation: &'static str,
    mut stream: Streaming<ServicingResponse>,
) -> Result<CompletedResponse, TridentClientError> {
    while let Some(item) = stream.next().await {
        let response = item.map_err(|source| TridentClientError::Stream {
            operation,
            source: anyhow!(source),
        })?;

        match response.response {
            Some(ResponseBody::Started(_)) => {
                log::info!("[Trident:{operation}] started");
            }
            Some(ResponseBody::Log(log_record)) => {
                let msg = format!("[Trident:{operation}] {}", log_record.message);
                match log_record.level() {
                    LogLevel::Unspecified | LogLevel::Trace => log::trace!("{msg}"),
                    LogLevel::Debug => log::debug!("{msg}"),
                    LogLevel::Info => log::info!("{msg}"),
                    LogLevel::Warn => log::warn!("{msg}"),
                    LogLevel::Error => log::error!("{msg}"),
                }
            }
            Some(ResponseBody::Completed(completed)) => {
                if completed.status() == StatusCode::Success {
                    return Ok(CompletedResponse {
                        reboot_status: completed.reboot_status(),
                        servicing_kind: completed
                            .servicing_kind
                            .and_then(|value| ServicingKind::try_from(value).ok()),
                    });
                }

                let details = completed
                    .error
                    .map(|error| RemoteError {
                        kind: TridentErrorKind::try_from(error.kind).ok(),
                        subkind: error.subkind,
                        message: error.message,
                        error_message: error.error_message,
                    })
                    .unwrap_or(RemoteError {
                        kind: None,
                        subkind: "unknown".to_string(),
                        message: format!("Trident {operation} failed without structured error"),
                        error_message: String::new(),
                    });
                return Err(TridentClientError::Remote { operation, details });
            }
            None => continue,
        }
    }

    Err(TridentClientError::MissingCompletion { operation })
}
