use std::ops::ControlFlow;

use log::{info, log, Level};
use tonic::{transport::Channel, Request, Streaming};

use harpoon::{
    servicing_response::Response as ResponseBody, trident_service_client::TridentServiceClient,
    CommitRequest, FinalizeRequest, LogLevel, ServicingRequest, ServicingResponse, StageRequest,
    StatusCode, StreamImageRequest, VersionRequest,
};
use url::Url;

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

impl RebootHandling {
    fn orchestrator_handles_reboot(self) -> bool {
        matches!(self, RebootHandling::Orchestrator)
    }
}

/// Client for interacting with the Trident gRPC server.
pub struct TridentClient {
    client: TridentServiceClient<Channel>,
}

impl TridentClient {
    /// Create a new TridentClient connected to the specified server address.
    pub async fn connect(server_address: &str) -> Result<Self, TridentClientError> {
        let client = TridentServiceClient::connect(server_address.to_string())
            .await
            .map_err(|e| TridentClientError::ConnectionError(server_address.to_string(), e))?;

        Ok(Self { client })
    }

    /// Get the version of the connected Trident server.
    pub async fn version(&mut self) -> Result<String, TridentClientError> {
        let request = Request::new(VersionRequest {});

        let response = self
            .client
            .version(request)
            .await
            .map_err(|e| TridentClientError::RequestError("version".to_string(), e))?;

        Ok(response.into_inner().version)
    }

    /// Install an image on the Trident server.
    pub async fn install(
        &mut self,
        host_configuration: impl Into<String>,
        reboot_handling: RebootHandling,
    ) -> Result<ExitKind, TridentClientError> {
        let request = Request::new(ServicingRequest {
            stage: Some(StageRequest {
                config: host_configuration.into(),
            }),
            finalize: Some(FinalizeRequest {
                orchestrator_handles_reboot: reboot_handling.orchestrator_handles_reboot(),
            }),
        });

        let response = self
            .client
            .install(request)
            .await
            .map_err(|e| TridentClientError::RequestError("install".to_string(), e))?
            .into_inner();

        handle_servicing_response_stream(response).await
    }

    /// Stream an image to the Trident server.
    pub async fn stream_image(
        &mut self,
        image_url: &Url,
        image_hash: impl Into<String>,
        reboot_handling: RebootHandling,
    ) -> Result<ExitKind, TridentClientError> {
        let request = Request::new(StreamImageRequest {
            image_path: image_url.to_string(),
            image_hash: image_hash.into(),
            finalize: Some(FinalizeRequest {
                orchestrator_handles_reboot: reboot_handling.orchestrator_handles_reboot(),
            }),
        });

        let response = self
            .client
            .stream_image(request)
            .await
            .map_err(|e| TridentClientError::RequestError("stream_image".to_string(), e))?
            .into_inner();

        handle_servicing_response_stream(response).await
    }

    /// Perform a commit on the Trident server.
    pub async fn commit(&mut self) -> Result<ExitKind, TridentClientError> {
        let response = self
            .client
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

            let target = format!("REMOTE::{}", log_entry.target);

            log!(target: &target, log_level, "{}", log_entry.message);
        }
        ResponseBody::FinalStatus(final_status) => {
            match (final_status.status(), final_status.error) {
                (StatusCode::Unspecified, Some(err)) => {
                    return Err(TridentClientError::InvalidResponse(format!(
                        "Unspecified final status with error: {}:{}: {}",
                        err.kind().as_str_name(),
                        err.subkind,
                        err.full_body,
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
                        err.full_body,
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

            if final_status.reboot_enqueued {
                info!("A reboot has been enqueued by Trident");
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
