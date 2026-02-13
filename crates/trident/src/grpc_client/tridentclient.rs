use std::ops::ControlFlow;

use log::{info, log, Level};
use tonic::{transport::Channel, Request, Streaming};
use url::Url;

use harpoon::v1::{
    servicing_response::Response as ResponseBody, streaming_service_client::StreamingServiceClient,
    version_service_client::VersionServiceClient, LogLevel, RebootHandling as ProtoRebootHandling,
    RebootManagement, ServicingResponse, StatusCode, StreamDiskRequest, VersionRequest,
};
#[cfg(feature = "grpc-preview")]
use harpoon::v1preview::{
    commit_service_client::CommitServiceClient, install_service_client::InstallServiceClient,
    rebuild_raid_service_client::RebuildRaidServiceClient,
    rollback_service_client::RollbackServiceClient, status_service_client::StatusServiceClient,
    update_service_client::UpdateServiceClient, validation_service_client::ValidationServiceClient,
    CommitRequest, FinalizeInstallRequest, HostConfiguration, InstallRequest, StageInstallRequest,
};

use crate::ExitKind;

use super::error::TridentClientError;

/// Indicates who is responsible for handling any required reboots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RebootHandling {
    /// The orchestrator is responsible for handling reboots.
    #[expect(dead_code)]
    Orchestrator,

    /// Trident is responsible for handling reboots.
    Trident,
}

impl From<RebootHandling> for ProtoRebootHandling {
    fn from(handler: RebootHandling) -> Self {
        match handler {
            RebootHandling::Orchestrator => Self::CallerHandlesReboot,
            RebootHandling::Trident => Self::AutoReboot,
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
    #[cfg(feature = "grpc-preview")]
    install_client: InstallServiceClient<Channel>,
    #[cfg(feature = "grpc-preview")]
    commit_client: CommitServiceClient<Channel>,

    #[expect(dead_code)]
    #[cfg(feature = "grpc-preview")]
    update_client: UpdateServiceClient<Channel>,
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
        let channel = Channel::from_shared(server_address.as_ref().to_string())
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
            commit_client: CommitServiceClient::new(channel.clone()),
            #[cfg(feature = "grpc-preview")]
            update_client: UpdateServiceClient::new(channel.clone()),
            #[cfg(feature = "grpc-preview")]
            rollback_client: RollbackServiceClient::new(channel.clone()),
            #[cfg(feature = "grpc-preview")]
            rebuild_raid_client: RebuildRaidServiceClient::new(channel.clone()),
            #[cfg(feature = "grpc-preview")]
            status_client: StatusServiceClient::new(channel.clone()),
            #[cfg(feature = "grpc-preview")]
            validation_client: ValidationServiceClient::new(channel.clone()),

            // Prod clients
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
        use harpoon::v1::RebootManagement;

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
    #[cfg(feature = "grpc-preview")]
    pub async fn commit(&mut self) -> Result<ExitKind, TridentClientError> {
        let response = self
            .commit_client
            .commit(Request::new(CommitRequest {}))
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

    Ok(ExitKind::Done)
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
        ResponseBody::Start(_) => info!("Servicing started"),
        ResponseBody::Log(log_entry) => {
            let log_level = match log_entry.level() {
                LogLevel::Unspecified => Level::Info,
                LogLevel::Trace => Level::Trace,
                LogLevel::Debug => Level::Debug,
                LogLevel::Info => Level::Info,
                LogLevel::Warn => Level::Warn,
                LogLevel::Error => Level::Error,
            };

            let target = format!("DAEMON::{}", log_entry.target);

            log!(target: &target, log_level, "{}", log_entry.message);
        }
        ResponseBody::FinalStatus(final_status) => {
            match (final_status.status(), final_status.error) {
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

            if final_status.reboot_started {
                info!("A reboot has been started by Trident");
            }

            if final_status.reboot_required {
                info!("A reboot is required to complete the operation");
                return Ok(ControlFlow::Break(ExitKind::NeedsReboot));
            }

            return Ok(ControlFlow::Break(ExitKind::Done));
        }
    }

    Ok(ControlFlow::Continue(()))
}
