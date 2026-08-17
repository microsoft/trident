//! gRPC helpers for talking to `tridentd`.
//!
//! Implements the Trident-invocation half of `docs/update-trigger-design.md`:
//! https://msazure.visualstudio.com/One/_git/Compute-ACL-Update-Service?version=GC1cfe79ec53bfc6936771e2433cba3dec0906b4fd&path=/docs/update-trigger-design.md
//! (the "Trident invocation" column of section 2.1's operations table,
//! and the stage/finalize/rollback-finalize CallerHandlesReboot split in
//! section 2.3).
//!
//! The annotation protocol drives stage/finalize/commit directly against
//! tridentd's stable v1 API (§4–§5). Startup recovery no longer pre-queries
//! the preview `StatusService::GetServicingState`: commit() is self-checking
//! (tridentd only commits from a valid servicing_state and otherwise returns
//! ServicingKind::NoneRequired as a harmless no-op), so the orchestrator
//! always calls commit() unconditionally and falls back to annotation-based
//! progress for anything commit() reports nothing to do for. See
//! orchestrator.rs's recover_from_trident_state for the full rationale.

use std::time::Duration;

use anyhow::anyhow;
use futures::StreamExt;
use tonic::{transport::Endpoint, Request, Streaming};
use trident_proto::v1::{
    commit_service_client::CommitServiceClient, rollback_service_client::RollbackServiceClient,
    servicing_response::Response as ResponseBody, update_service_client::UpdateServiceClient,
    CommitRequest, FinalizeUpdateRequest, HostConfiguration, LogLevel, ManualRollbackKind,
    RebootHandling, RebootManagement, RebootStatus, RollbackFinalizeRequest, RollbackStageRequest,
    ServicingKind, ServicingResponse, StageUpdateRequest, StatusCode, TridentErrorKind,
    UpdateRequest,
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
    rollback_client: RollbackServiceClient<tonic::transport::Channel>,
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

        Ok(Self::from_channel(channel))
    }

    /// Builds a client directly from an existing tonic Channel, bypassing
    /// socket/URI resolution entirely. Production code always goes through
    /// connect(); this exists so tests can hand the client a channel wired
    /// to an in-process fake tridentd (e.g. via Endpoint::connect_with_connector
    /// over an in-memory duplex stream) and exercise the exact same
    /// request/response/error-mapping code as production, without a real
    /// unix socket or subprocess.
    pub fn from_channel(channel: tonic::transport::Channel) -> Self {
        Self {
            update_client: UpdateServiceClient::new(channel.clone()),
            commit_client: CommitServiceClient::new(channel.clone()),
            rollback_client: RollbackServiceClient::new(channel),
        }
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
                    // The agent, not tridentd, must own every reboot
                    // decision: AKS-RP is the sole authority over
                    // reboot/rollback (accepted-design-v2.md §2.5). If commit()
                    // ever reports NeedsReboot (e.g. a health-check failure,
                    // were health checks ever re-enabled), the agent needs
                    // to see that as a RebootRequired response it controls
                    // and reports via labels, not have tridentd reboot out
                    // from under it.
                    handling: RebootHandling::CallerHandlesReboot.into(),
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

    /// Stages an A/B rollback. Only `AbRollbackRequested` is used - per the
    /// accepted design, trident-acl-agent only ever drives AB-kind manual
    /// rollback; runtime-kind and "any" rollback are out of scope for the
    /// annotation-driven protocol.
    pub async fn rollback_stage(
        &mut self,
        timeout: Duration,
    ) -> Result<CompletedResponse, TridentClientError> {
        let response = self
            .rollback_client
            .rollback_stage(Request::new(RollbackStageRequest {
                kind: ManualRollbackKind::AbRollbackRequested.into(),
            }))
            .await
            .map_err(|source| TridentClientError::Request {
                operation: "rollback_stage",
                source,
            })?
            .into_inner();

        run_with_timeout(
            "rollback_stage",
            timeout,
            consume_servicing_stream("rollback_stage", response),
        )
        .await
    }

    pub async fn rollback_finalize(
        &mut self,
        timeout: Duration,
    ) -> Result<CompletedResponse, TridentClientError> {
        let response = self
            .rollback_client
            .rollback_finalize(Request::new(RollbackFinalizeRequest {
                reboot: Some(RebootManagement {
                    // Same rationale as commit()/update_finalize(): AKS-RP,
                    // via the agent, is the sole authority over reboot
                    // timing (accepted-design-v2.md §2.5).
                    handling: RebootHandling::CallerHandlesReboot.into(),
                }),
            }))
            .await
            .map_err(|source| TridentClientError::Request {
                operation: "rollback_finalize",
                source,
            })?
            .into_inner();

        run_with_timeout(
            "rollback_finalize",
            timeout,
            consume_servicing_stream("rollback_finalize", response),
        )
        .await
    }
}

#[derive(serde::Serialize)]
struct ImageSpec<'a> {
    url: &'a str,
    sha384: &'a str,
}

#[derive(serde::Serialize)]
struct HostConfigurationYaml<'a> {
    image: ImageSpec<'a>,
}

pub fn host_configuration_from_image(url: &Url, hash: Option<&str>) -> HostConfiguration {
    // Build via serde_yaml rather than raw string formatting so a URL or
    // hash containing YAML-special characters (e.g. ':' or '#') can't
    // produce invalid YAML or silently change the parsed structure fed to
    // tridentd as configuration.
    let spec = HostConfigurationYaml {
        image: ImageSpec {
            url: url.as_str(),
            sha384: hash.unwrap_or("ignored"),
        },
    };
    HostConfiguration {
        config: serde_yaml::to_string(&spec)
            .expect("serializing a simple struct to YAML cannot fail"),
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
