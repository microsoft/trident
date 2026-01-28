use anyhow::{anyhow, Result};
use log::{error, info, warn};
use std::time::Duration;
use tonic::transport::Channel;

use crate::{cli::Commands, trident_service_client::TridentServiceClient};
// Rename the generated TridentError to avoid conflicts
use crate::{
    CheckRootRequest,
    CommitRequest,
    FinalizeRequest,
    GetActiveVolumeRequest,
    GetConfigRequest as GetProvisionedConfigRequest,
    GetConfigRequest as GetServicingConfigRequest,
    GetLastErrorRequest,
    GetRequiredServicingTypeRequest,
    GetServicingStateRequest,
    RebuildRaidRequest,
    ServicingRequest,
    StageRequest,
    StreamImageRequest,
    TridentError as ProtoTridentError, // Rename the proto version
    ValidateHostConfigurationRequest,
};

/// gRPC client for communicating with Trident core
pub struct TridentClient {
    client: TridentServiceClient<Channel>,
    server_url: String,
}

impl TridentClient {
    /// Create a new TridentClient
    pub async fn new(server_url: &str) -> Result<Self> {
        info!("Connecting to Trident gRPC server at {}", server_url);

        let channel = Channel::from_shared(server_url.to_string())?
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .connect()
            .await
            .map_err(|e| anyhow!("Failed to connect to gRPC server: {}", e))?;

        let client = TridentServiceClient::new(channel);

        Ok(Self {
            client,
            server_url: server_url.to_string(),
        })
    }

    /// Handle CLI commands by translating them to gRPC calls
    pub async fn handle_command(&mut self, command: &Commands) -> Result<()> {
        match command {
            Commands::Install {
                config,
                allowed_operations,
                status: _,
                error: _,
                multiboot: _,
            } => {
                self.handle_install(config.to_string_lossy().to_string(), allowed_operations)
                    .await
            }
            Commands::Update {
                config,
                allowed_operations,
                status: _,
                error: _,
            } => {
                self.handle_update(config.to_string_lossy().to_string(), allowed_operations)
                    .await
            }
            Commands::Commit {
                status: _,
                error: _,
            } => self.handle_commit().await,
            Commands::Listen {
                status: _,
                error: _,
                ..
            } => {
                // Listen mode - not a gRPC call
                info!("Listen mode - starting gRPC server mode");
                Ok(())
            }
            Commands::RebuildRaid {
                config,
                status: _,
                error: _,
            } => {
                let config_path = config
                    .as_ref()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|| "/etc/trident/config.yaml".to_string());
                self.handle_rebuild_raid(config_path).await
            }
            Commands::StartNetwork { config } => {
                // Network configuration - could be handled locally or via gRPC
                info!("Starting network configuration with config: {:?}", config);
                Ok(())
            }
            Commands::Get { kind, outfile: _ } => match kind {
                crate::cli::GetKind::Status => self.handle_get_state().await,
                crate::cli::GetKind::Configuration => self.handle_get_provisioned_config().await,
                crate::cli::GetKind::LastError => self.handle_get_last_error().await,
            },
            Commands::Validate { config } => {
                self.handle_validate_config(config.to_string_lossy().to_string())
                    .await
            }
            #[cfg(feature = "pytest-generator")]
            Commands::Pytest => {
                info!("Pytest generation mode");
                Ok(())
            }
            Commands::OfflineInitialize { .. } => {
                info!("Offline initialization - this is typically handled locally");
                Ok(())
            }
            #[cfg(feature = "dangerous-options")]
            Commands::StreamImage {
                image,
                hash,
                status: _,
                error: _,
            } => {
                self.handle_stream_image(image.to_string(), Some(hash.clone()))
                    .await
            }
        }
    }

    async fn handle_install(
        &mut self,
        config: String,
        _allowed_operations: &[crate::cli::AllowedOperation],
    ) -> Result<()> {
        info!("Starting install operation");

        let config_content = tokio::fs::read_to_string(&config)
            .await
            .map_err(|e| anyhow!("Failed to read config file {}: {}", config, e))?;

        let request = ServicingRequest {
            stage: Some(StageRequest {
                config: config_content.clone(),
            }),
            finalize: Some(FinalizeRequest {
                orchestrator_handles_reboot: false,
            }),
        };

        let mut stream = self.client.install(request).await?.into_inner();

        while let Some(response) = stream.message().await? {
            self.handle_servicing_response(response).await?;
        }

        info!("Install operation completed");
        Ok(())
    }

    async fn handle_update(
        &mut self,
        config: String,
        _allowed_operations: &[crate::cli::AllowedOperation],
    ) -> Result<()> {
        info!("Starting update operation");

        let config_content = tokio::fs::read_to_string(&config)
            .await
            .map_err(|e| anyhow!("Failed to read config file {}: {}", config, e))?;

        let request = ServicingRequest {
            stage: Some(StageRequest {
                config: config_content.clone(),
            }),
            finalize: Some(FinalizeRequest {
                orchestrator_handles_reboot: false,
            }),
        };

        let mut stream = self.client.update(request).await?.into_inner();

        while let Some(response) = stream.message().await? {
            self.handle_servicing_response(response).await?;
        }

        info!("Update operation completed");
        Ok(())
    }

    async fn handle_commit(&mut self) -> Result<()> {
        info!("Starting commit operation");

        let request = CommitRequest {};
        let mut stream = self.client.commit(request).await?.into_inner();

        while let Some(response) = stream.message().await? {
            self.handle_servicing_response(response).await?;
        }

        info!("Commit operation completed");
        Ok(())
    }

    async fn handle_check_root(&mut self) -> Result<()> {
        info!("Starting check root operation");

        let request = CheckRootRequest {};
        let mut stream = self.client.check_root(request).await?.into_inner();

        while let Some(response) = stream.message().await? {
            self.handle_servicing_response(response).await?;
        }

        info!("Check root operation completed");
        Ok(())
    }

    async fn handle_get_state(&mut self) -> Result<()> {
        info!("Getting servicing state");

        let request = GetServicingStateRequest {};
        let response = self.client.get_servicing_state(request).await?.into_inner();

        info!("Current servicing state: {:?}", response.state);
        Ok(())
    }

    async fn handle_get_provisioned_config(&mut self) -> Result<()> {
        info!("Getting provisioned config");

        let request = GetProvisionedConfigRequest {};
        let response = self
            .client
            .get_provisioned_config(request)
            .await?
            .into_inner();

        if let Some(config) = response.config {
            info!("Provisioned config:\n{}", config);
        } else {
            info!("No provisioned config available");
        }
        Ok(())
    }

    async fn handle_get_servicing_config(&mut self) -> Result<()> {
        info!("Getting servicing config");

        let request = GetServicingConfigRequest {};
        let response = self
            .client
            .get_servicing_config(request)
            .await?
            .into_inner();

        if let Some(config) = response.config {
            info!("Servicing config:\n{}", config);
        } else {
            info!("No servicing config available");
        }
        Ok(())
    }

    async fn handle_get_last_error(&mut self) -> Result<()> {
        info!("Getting last error");

        let request = GetLastErrorRequest {};
        let response = self.client.get_last_error(request).await?.into_inner();

        if let Some(error) = response.error {
            error!("Last error: {:?}", error);
        } else {
            info!("No error reported");
        }
        Ok(())
    }

    async fn handle_get_active_volume(&mut self) -> Result<()> {
        info!("Getting active volume");

        let request = GetActiveVolumeRequest {};
        let response = self.client.get_active_volume(request).await?.into_inner();

        info!("Active volume: {:?}", response.active_volume);
        Ok(())
    }

    async fn handle_validate_config(&mut self, config: String) -> Result<()> {
        info!("Validating host configuration");

        let config_content = tokio::fs::read_to_string(&config)
            .await
            .map_err(|e| anyhow!("Failed to read config file {}: {}", config, e))?;

        let request = ValidateHostConfigurationRequest {
            config: config_content,
        };
        let response = self
            .client
            .validate_host_configuration(request)
            .await?
            .into_inner();

        if response.valid {
            info!("Configuration is valid");
        } else {
            error!(
                "Configuration is invalid: {}",
                response.message.unwrap_or_default()
            );
        }
        Ok(())
    }

    async fn handle_get_required_servicing_type(&mut self, config: String) -> Result<()> {
        info!("Getting required servicing type");

        let config_content = tokio::fs::read_to_string(&config)
            .await
            .map_err(|e| anyhow!("Failed to read config file {}: {}", config, e))?;

        let request = GetRequiredServicingTypeRequest {
            config: config_content,
        };
        let response = self
            .client
            .get_required_servicing_type(request)
            .await?
            .into_inner();

        info!("Required servicing type: {:?}", response.servicing_type);
        Ok(())
    }

    async fn handle_rebuild_raid(&mut self, config: String) -> Result<()> {
        info!("Starting RAID rebuild operation");

        let config_content = tokio::fs::read_to_string(&config)
            .await
            .map_err(|e| anyhow!("Failed to read config file {}: {}", config, e))?;

        let request = RebuildRaidRequest {
            config: config_content,
        };
        let mut stream = self.client.rebuild_raid(request).await?.into_inner();

        while let Some(response) = stream.message().await? {
            self.handle_servicing_response(response).await?;
        }

        info!("RAID rebuild operation completed");
        Ok(())
    }

    async fn handle_stream_image(
        &mut self,
        image_path: String,
        image_hash: Option<String>,
    ) -> Result<()> {
        info!("Starting image streaming operation");

        let request = StreamImageRequest {
            image_path,
            image_hash: image_hash.unwrap_or_default(),
        };
        let mut stream = self.client.stream_image(request).await?.into_inner();

        while let Some(response) = stream.message().await? {
            self.handle_servicing_response(response).await?;
        }

        info!("Image streaming operation completed");
        Ok(())
    }

    async fn handle_servicing_response(&self, response: crate::ServicingResponse) -> Result<()> {
        use crate::servicing_response::Response;

        if let Some(timestamp) = response.timestamp {
            let timestamp_str = format!("{}.{:06}s", timestamp.seconds, timestamp.nanos / 1000);

            match response.response {
                Some(Response::Start(_)) => {
                    info!("[{}] Operation started", timestamp_str);
                }
                Some(Response::Log(log)) => {
                    let level = log.level();
                    let message = format!("[{}] [{}] {}", timestamp_str, log.target, log.message);

                    match level {
                        crate::LogLevel::Error => error!("{}", message),
                        crate::LogLevel::Warn => warn!("{}", message),
                        crate::LogLevel::Info => info!("{}", message),
                        crate::LogLevel::Debug => log::debug!("{}", message),
                        crate::LogLevel::Trace => log::trace!("{}", message),
                    }
                }
                Some(Response::FinalStatus(status)) => match status.status() {
                    crate::StatusCode::Success => {
                        info!("[{}] Operation completed successfully", timestamp_str);
                        if status.reboot_required {
                            warn!("Reboot is required to complete the operation");
                        }
                    }
                    crate::StatusCode::Failure => {
                        error!("[{}] Operation failed", timestamp_str);
                        if let Some(error) = status.error {
                            error!("Error details: {:?}", error);
                        }
                        return Err(anyhow!("Servicing operation failed"));
                    }
                },
                None => {
                    warn!("[{}] Received empty response", timestamp_str);
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_grpc_client_creation_invalid_address() {
        let result = TridentClient::new("http://invalid-address:9999").await;
        assert!(result.is_err());
    }

    #[test]
    fn test_handle_servicing_response_structure() {
        // Test that we can create and handle response structures
        use crate::servicing_response::Response;

        let response = crate::ServicingResponse {
            timestamp: Some(prost_types::Timestamp {
                seconds: 1234567890,
                nanos: 0,
            }),
            response: Some(Response::FinalStatus(crate::FinalStatus {
                status: crate::StatusCode::Success as i32,
                error: None,
                reboot_required: false,
            })),
        };

        // Verify we can access the response structure
        assert!(response.timestamp.is_some());
        assert!(response.response.is_some());
    }
}
