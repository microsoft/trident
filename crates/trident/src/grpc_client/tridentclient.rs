use std::ops::ControlFlow;

use log::{debug, info};
use tonic::{
    transport::{Channel, Endpoint},
    Request, Streaming,
};
use url::Url;

use trident_proto::v1::{
    commit_service_client::CommitServiceClient, servicing_response::Response as ResponseBody,
    streaming_service_client::StreamingServiceClient, update_service_client::UpdateServiceClient,
    version_service_client::VersionServiceClient, CommitRequest, FinalizeUpdateRequest,
    HostConfiguration, RebootHandling as ProtoRebootHandling, RebootManagement, RebootStatus,
    ServicingResponse, StageUpdateRequest, StatusCode, StreamDiskRequest, UpdateRequest,
    VersionRequest,
};

#[cfg(feature = "grpc-preview")]
use trident_proto::v1preview::{
    install_service_client::InstallServiceClient,
    rebuild_raid_service_client::RebuildRaidServiceClient,
    rollback_service_client::RollbackServiceClient, status_service_client::StatusServiceClient,
    validation_service_client::ValidationServiceClient, FinalizeInstallRequest, InstallRequest,
    StageInstallRequest,
};

use crate::ExitKind;

use super::error::TridentClientError;

/// Indicates who is responsible for handling any required reboots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RebootHandling {
    /// The orchestrator is responsible for handling reboots.
    Orchestrator,

    /// Trident is responsible for handling reboots.
    Trident,
}

impl From<RebootHandling> for ProtoRebootHandling {
    fn from(handler: RebootHandling) -> Self {
        match handler {
            RebootHandling::Orchestrator => Self::CallerHandlesReboot,
            RebootHandling::Trident => Self::TridentHandlesReboot,
        }
    }
}

impl From<RebootHandling> for i32 {
    fn from(handler: RebootHandling) -> Self {
        ProtoRebootHandling::from(handler) as i32
    }
}

/// Client for interacting with the Trident gRPC server.
pub struct TridentClient {
    version_client: VersionServiceClient<Channel>,
    streaming_client: StreamingServiceClient<Channel>,
    update_client: UpdateServiceClient<Channel>,
    commit_client: CommitServiceClient<Channel>,

    #[cfg(feature = "grpc-preview")]
    install_client: InstallServiceClient<Channel>,
    #[expect(dead_code)]
    #[cfg(feature = "grpc-preview")]
    rollback_client: RollbackServiceClient<Channel>,
    #[expect(dead_code)]
    #[cfg(feature = "grpc-preview")]
    rebuild_raid_client: RebuildRaidServiceClient<Channel>,
    #[expect(dead_code)]
    #[cfg(feature = "grpc-preview")]
    status_client: StatusServiceClient<Channel>,
    #[expect(dead_code)]
    #[cfg(feature = "grpc-preview")]
    validation_client: ValidationServiceClient<Channel>,
}

impl TridentClient {
    /// Create a new TridentClient connected to the specified server address.
    pub async fn connect(server_address: impl AsRef<str>) -> Result<Self, TridentClientError> {
        let channel = Endpoint::new(server_address.as_ref().to_string())
            .map_err(|e| {
                TridentClientError::InvalidServerAddress(
                    server_address.as_ref().to_string(),
                    e.to_string(),
                )
            })?
            .connect()
            .await
            .map_err(|e| {
                TridentClientError::ConnectionError(server_address.as_ref().to_string(), e)
            })?;

        Ok(Self {
            #[cfg(feature = "grpc-preview")]
            install_client: InstallServiceClient::new(channel.clone()),
            #[cfg(feature = "grpc-preview")]
            rollback_client: RollbackServiceClient::new(channel.clone()),
            #[cfg(feature = "grpc-preview")]
            rebuild_raid_client: RebuildRaidServiceClient::new(channel.clone()),
            #[cfg(feature = "grpc-preview")]
            status_client: StatusServiceClient::new(channel.clone()),
            #[cfg(feature = "grpc-preview")]
            validation_client: ValidationServiceClient::new(channel.clone()),

            // Prod clients
            commit_client: CommitServiceClient::new(channel.clone()),
            update_client: UpdateServiceClient::new(channel.clone()),
            version_client: VersionServiceClient::new(channel.clone()),
            streaming_client: StreamingServiceClient::new(channel),
        })
    }

    /// Get the version of the connected Trident server.
    pub async fn version(&mut self) -> Result<String, TridentClientError> {
        let request = Request::new(VersionRequest {});

        let response = self
            .version_client
            .version(request)
            .await
            .map_err(|e| TridentClientError::RequestError("version".to_string(), e))?;

        Ok(response.into_inner().version)
    }

    /// Install an image on the Trident server.
    #[cfg(feature = "grpc-preview")]
    pub async fn install(
        &mut self,
        host_configuration: impl Into<String>,
        reboot_handling: RebootHandling,
    ) -> Result<ExitKind, TridentClientError> {
        let request = Request::new(InstallRequest {
            stage: Some(StageInstallRequest {
                config: Some(HostConfiguration {
                    config: host_configuration.into(),
                }),
            }),
            finalize: Some(FinalizeInstallRequest {
                reboot: Some(RebootManagement {
                    handling: reboot_handling.into(),
                }),
            }),
        });

        let response = self
            .install_client
            .install(request)
            .await
            .map_err(|e| TridentClientError::RequestError("install".to_string(), e))?
            .into_inner();

        handle_servicing_response_stream(response).await
    }

    pub async fn update(
        &mut self,
        host_configuration: impl Into<String>,
        reboot_handling: RebootHandling,
    ) -> Result<ExitKind, TridentClientError> {
        let request = Request::new(UpdateRequest {
            stage: Some(StageUpdateRequest {
                config: Some(HostConfiguration {
                    config: host_configuration.into(),
                }),
            }),
            finalize: Some(FinalizeUpdateRequest {
                reboot: Some(RebootManagement {
                    handling: reboot_handling.into(),
                }),
            }),
        });

        let response = self
            .update_client
            .update(request)
            .await
            .map_err(|e| TridentClientError::RequestError("update".to_string(), e))?
            .into_inner();

        handle_servicing_response_stream(response).await
    }

    pub async fn update_stage(
        &mut self,
        host_configuration: impl Into<String>,
    ) -> Result<ExitKind, TridentClientError> {
        let request = Request::new(StageUpdateRequest {
            config: Some(HostConfiguration {
                config: host_configuration.into(),
            }),
        });

        let response = self
            .update_client
            .update_stage(request)
            .await
            .map_err(|e| TridentClientError::RequestError("update_stage".to_string(), e))?
            .into_inner();

        handle_servicing_response_stream(response).await
    }

    pub async fn update_finalize(
        &mut self,
        reboot_handling: RebootHandling,
    ) -> Result<ExitKind, TridentClientError> {
        let request = Request::new(FinalizeUpdateRequest {
            reboot: Some(RebootManagement {
                handling: reboot_handling.into(),
            }),
        });

        let response = self
            .update_client
            .update_finalize(request)
            .await
            .map_err(|e| TridentClientError::RequestError("update_finalize".to_string(), e))?
            .into_inner();

        handle_servicing_response_stream(response).await
    }

    /// Stream an image to the Trident server.
    pub async fn stream_disk(
        &mut self,
        image_url: &Url,
        image_hash: Option<impl Into<String>>,
        reboot_handling: RebootHandling,
    ) -> Result<ExitKind, TridentClientError> {
        let request = Request::new(StreamDiskRequest {
            image_url: image_url.to_string(),
            image_hash: image_hash.map(|h| h.into()),
            reboot: Some(RebootManagement {
                handling: reboot_handling.into(),
            }),
        });

        let response = self
            .streaming_client
            .stream_disk(request)
            .await
            .map_err(|e| TridentClientError::RequestError("stream_disk".to_string(), e))?
            .into_inner();

        handle_servicing_response_stream(response).await
    }

    /// Perform a commit on the Trident server.
    pub async fn commit(&mut self) -> Result<ExitKind, TridentClientError> {
        let response = self
            .commit_client
            .commit(Request::new(CommitRequest {
                reboot: Some(RebootManagement {
                    handling: RebootHandling::Trident.into(),
                }),
            }))
            .await
            .map_err(|e| TridentClientError::RequestError("commit".to_string(), e))?
            .into_inner();

        handle_servicing_response_stream(response).await
    }
}

async fn handle_servicing_response_stream(
    mut stream: Streaming<ServicingResponse>,
) -> Result<ExitKind, TridentClientError> {
    loop {
        match stream.message().await {
            Ok(Some(msg)) => match handle_servicing_response(msg).await? {
                ControlFlow::Continue(()) => continue,
                ControlFlow::Break(kind) => return Ok(kind),
            },
            Ok(None) => break, // End of stream
            Err(e) => {
                return Err(TridentClientError::ResponseError(
                    "servicing stream".to_string(),
                    e,
                ));
            }
        }
    }

    Err(TridentClientError::ResponseError(
        "servicing stream".to_string(),
        tonic::Status::unavailable("Servicing stream ended before the final response"),
    ))
}

async fn handle_servicing_response(
    msg: ServicingResponse,
) -> Result<ControlFlow<ExitKind, ()>, TridentClientError> {
    let Some(body) = msg.response else {
        return Err(TridentClientError::InvalidResponse(
            "Missing body in servicing response".to_string(),
        ));
    };

    match body {
        ResponseBody::Started(_) => info!("Servicing started"),
        ResponseBody::Log(log_entry) => {
            log_entry.log(Some("DAEMON"));
        }
        ResponseBody::Completed(status) => {
            match (status.status(), status.error.as_ref()) {
                (StatusCode::Unspecified, Some(err)) => {
                    return Err(TridentClientError::InvalidResponse(format!(
                        "Unspecified final status with error: {}:{}: {}",
                        err.kind().as_str_name(),
                        err.subkind,
                        err.message,
                    )));
                }
                (StatusCode::Unspecified, None) => {
                    return Err(TridentClientError::InvalidResponse(
                        "Unspecified final status without error".to_string(),
                    ));
                }
                (StatusCode::Failure, Some(err)) => {
                    return Err(TridentClientError::ServicingError(format!(
                        "Servicing failed with error: {}:{}: {}",
                        err.kind().as_str_name(),
                        err.subkind,
                        err.message,
                    )));
                }
                (StatusCode::Failure, None) => {
                    return Err(TridentClientError::ServicingError(
                        "Servicing failed without error".to_string(),
                    ));
                }
                (StatusCode::Success, _) => {}
            }

            info!("Servicing completed successfully");

            match status.reboot_status() {
                RebootStatus::RebootStarted => {
                    info!("A reboot has been started by Trident");
                    // IMPORTANT: This message is used by E2E A/B update tests to validate that
                    // a reboot was requested. Do not change or remove without updating the
                    // test constant in:
                    // tools/storm/utils/trident/trident.go:REBOOTING_LOG_MESSAGE
                    debug!("Requesting reboot");
                }
                RebootStatus::RebootRequired => {
                    info!("A reboot is required to complete the operation");
                    return Ok(ControlFlow::Break(ExitKind::NeedsReboot));
                }
                RebootStatus::RebootNotRequired => {
                    info!("No reboot is required to complete the operation")
                }
                RebootStatus::Unspecified => {
                    return Err(TridentClientError::InvalidResponse(
                        "Unspecified reboot status in final response".to_string(),
                    ));
                }
            }

            return Ok(ControlFlow::Break(ExitKind::Done));
        }
    }

    Ok(ControlFlow::Continue(()))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use hyper::{body::Bytes, Response};
    use tokio::{net::TcpListener, sync::oneshot, task::JoinSet, time::timeout};

    use super::*;

    async fn assert_mid_stream_reset_is_transport_failure(
        reason: h2::Reason,
        expected_code: tonic::Code,
    ) {
        let mut server = JoinSet::new();
        timeout(Duration::from_secs(10), async {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let (reset, reset_requested) = oneshot::channel();
            server.spawn(async move {
                let (socket, _) = listener.accept().await.unwrap();
                let mut connection = h2::server::handshake(socket).await.unwrap();
                let (_request, mut respond) = connection.accept().await.unwrap().unwrap();
                let response = Response::builder()
                    .status(200)
                    .header("content-type", "application/grpc")
                    .body(())
                    .unwrap();
                let mut stream = respond.send_response(response, false).unwrap();
                // An empty protobuf message establishes a readable gRPC stream.
                stream
                    .send_data(Bytes::from_static(&[0, 0, 0, 0, 0]), false)
                    .unwrap();
                tokio::select! {
                    requested = reset_requested => {
                        requested.unwrap();
                        stream.send_reset(reason);
                    }
                    _ = connection.accept() => panic!("connection closed before the reset"),
                }
                while let Some(request) = connection.accept().await {
                    request.unwrap();
                }
            });

            let channel = Endpoint::from_shared(format!("http://{address}"))
                .unwrap()
                .connect()
                .await
                .unwrap();
            let mut client = StreamingServiceClient::new(channel);
            let mut stream = client
                .stream_disk(StreamDiskRequest::default())
                .await
                .unwrap()
                .into_inner();
            assert!(stream.message().await.unwrap().is_some());
            reset.send(()).unwrap();

            let error = handle_servicing_response_stream(stream).await.unwrap_err();
            let TridentClientError::ResponseError(_, ref status) = error else {
                panic!("expected a mid-stream response error: {error}");
            };
            assert_eq!(status.code(), expected_code);
            assert_eq!(
                std::error::Error::source(status).is_some(),
                reason != h2::Reason::CANCEL,
            );
            assert!(super::super::is_transport_failure(&Err(error.into())));
            drop(client);
            server.join_next().await.unwrap().unwrap();
        })
        .await
        .expect("the stream reset test must finish within its deadline");
    }

    #[tokio::test]
    async fn test_mid_stream_http2_cancel_is_a_transport_failure() {
        // Hyper 1.8.1 maps CANCEL to EOF instead of retaining a status source.
        assert_mid_stream_reset_is_transport_failure(h2::Reason::CANCEL, tonic::Code::Unavailable)
            .await;
    }

    #[tokio::test]
    async fn test_mid_stream_http2_internal_reset_is_a_transport_failure() {
        assert_mid_stream_reset_is_transport_failure(
            h2::Reason::INTERNAL_ERROR,
            tonic::Code::Internal,
        )
        .await;
    }

    #[tokio::test]
    async fn test_completed_servicing_response_preserves_exit_kind() {
        for reboot in [
            RebootStatus::RebootNotRequired,
            RebootStatus::RebootStarted,
            RebootStatus::RebootRequired,
        ] {
            let response = ServicingResponse {
                response: Some(ResponseBody::Completed(trident_proto::v1::Completed {
                    status: StatusCode::Success.into(),
                    reboot_status: reboot.into(),
                    ..Default::default()
                })),
                ..Default::default()
            };
            let result = handle_servicing_response(response).await.unwrap();
            if reboot == RebootStatus::RebootRequired {
                assert!(matches!(result, ControlFlow::Break(ExitKind::NeedsReboot)));
            } else {
                assert!(matches!(result, ControlFlow::Break(ExitKind::Done)));
            }
        }
    }
}
