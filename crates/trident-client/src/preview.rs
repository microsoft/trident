use tonic::Request;

use trident_api::config::HostConfiguration;
use trident_proto::v1preview::{
    install_service_client::InstallServiceClient,
    rebuild_raid_service_client::RebuildRaidServiceClient,
    rollback_service_client::RollbackServiceClient as PreviewRollbackServiceClient,
    status_service_client::StatusServiceClient, validation_service_client::ValidationServiceClient,
    CheckRollbackRequest, FinalizeInstallRequest, GetActiveVolumeRequest, GetServicingStateRequest,
    InstallRequest, RebuildRaidRequest, StageInstallRequest, ValidateHostConfigurationRequest,
};

use crate::{
    protocol::{self, Operation},
    Client, ClientError, RebootPolicy, RemoteError, RollbackKind, ServicingOperation,
};

/// Result of static Host Configuration validation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidationResult {
    pub valid: bool,
    pub error: Option<RemoteError>,
}

/// Rollback availability reported by preview rollback inspection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum RollbackAvailability {
    Unspecified,
    None,
    Ab,
    Runtime,
    Unknown(i32),
}

/// Current servicing state reported by `tridentd`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ServicingState {
    Unspecified,
    NotProvisioned,
    CleanInstallStaged,
    AbUpdateStaged,
    ManualRollbackAbStaged,
    ManualRollbackRuntimeStaged,
    RuntimeUpdateStaged,
    CleanInstallFinalized,
    AbUpdateFinalized,
    ManualRollbackAbFinalized,
    Provisioned,
    AbUpdateHealthCheckFailed,
    /// The trial OS booted, but hypervisor validation failed and automatic recovery is pending.
    AbUpdateHypervisorCheckFailed,
    Unknown(i32),
}

/// Active A/B volume reported by `tridentd`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ActiveVolume {
    Unspecified,
    None,
    VolumeA,
    VolumeB,
    Unknown(i32),
}

impl Client {
    /// Performs an install from a typed Host Configuration.
    pub async fn install(
        &self,
        config: &HostConfiguration,
        reboot: RebootPolicy,
    ) -> Result<ServicingOperation, ClientError> {
        self.install_yaml(
            protocol::serialize_config(Operation::Install, config)?,
            reboot,
        )
        .await
    }

    /// Performs an install using Host Configuration YAML unchanged.
    pub async fn install_yaml(
        &self,
        config: impl Into<String>,
        reboot: RebootPolicy,
    ) -> Result<ServicingOperation, ClientError> {
        let request = InstallRequest {
            stage: Some(StageInstallRequest {
                config: Some(protocol::proto_config(config)),
            }),
            finalize: Some(FinalizeInstallRequest {
                reboot: Some(protocol::reboot_management(reboot)),
            }),
        };
        let mut client = InstallServiceClient::new(self.channel.clone());
        self.servicing(Operation::Install, client.install(Request::new(request)))
            .await
    }

    /// Stages an install from a typed Host Configuration.
    pub async fn install_stage(
        &self,
        config: &HostConfiguration,
    ) -> Result<ServicingOperation, ClientError> {
        self.install_stage_yaml(protocol::serialize_config(Operation::InstallStage, config)?)
            .await
    }

    /// Stages an install using Host Configuration YAML unchanged.
    pub async fn install_stage_yaml(
        &self,
        config: impl Into<String>,
    ) -> Result<ServicingOperation, ClientError> {
        let request = StageInstallRequest {
            config: Some(protocol::proto_config(config)),
        };
        let mut client = InstallServiceClient::new(self.channel.clone());
        self.servicing(
            Operation::InstallStage,
            client.install_stage(Request::new(request)),
        )
        .await
    }

    /// Finalizes a staged install.
    pub async fn install_finalize(
        &self,
        reboot: RebootPolicy,
    ) -> Result<ServicingOperation, ClientError> {
        let request = FinalizeInstallRequest {
            reboot: Some(protocol::reboot_management(reboot)),
        };
        let mut client = InstallServiceClient::new(self.channel.clone());
        self.servicing(
            Operation::InstallFinalize,
            client.install_finalize(Request::new(request)),
        )
        .await
    }

    /// Rebuilds degraded RAID arrays.
    pub async fn rebuild_raid(&self) -> Result<ServicingOperation, ClientError> {
        let mut client = RebuildRaidServiceClient::new(self.channel.clone());
        self.servicing(
            Operation::RebuildRaid,
            client.rebuild_raid(Request::new(RebuildRaidRequest {})),
        )
        .await
    }

    /// Returns the current servicing state.
    pub async fn servicing_state(&self) -> Result<ServicingState, ClientError> {
        let mut client = StatusServiceClient::new(self.channel.clone());
        let response = self
            .unary(
                Operation::ServicingState,
                client.get_servicing_state(Request::new(GetServicingStateRequest {})),
            )
            .await?
            .into_inner();
        Ok(protocol::decode_servicing_state(response.state))
    }

    /// Returns the active A/B volume.
    pub async fn active_volume(&self) -> Result<ActiveVolume, ClientError> {
        let mut client = StatusServiceClient::new(self.channel.clone());
        let response = self
            .unary(
                Operation::ActiveVolume,
                client.get_active_volume(Request::new(GetActiveVolumeRequest {})),
            )
            .await?
            .into_inner();
        Ok(protocol::decode_active_volume(response.active_volume))
    }

    /// Validates a typed Host Configuration.
    pub async fn validate_host_configuration(
        &self,
        config: &HostConfiguration,
    ) -> Result<ValidationResult, ClientError> {
        self.validate_host_configuration_yaml(protocol::serialize_config(
            Operation::ValidateHostConfiguration,
            config,
        )?)
        .await
    }

    /// Validates Host Configuration YAML unchanged.
    pub async fn validate_host_configuration_yaml(
        &self,
        config: impl Into<String>,
    ) -> Result<ValidationResult, ClientError> {
        let request = ValidateHostConfigurationRequest {
            config: Some(protocol::proto_config(config)),
        };
        let mut client = ValidationServiceClient::new(self.channel.clone());
        let response = self
            .unary(
                Operation::ValidateHostConfiguration,
                client.validate_host_configuration(Request::new(request)),
            )
            .await?
            .into_inner();
        protocol::decode_validation_result(response.ok, response.error)
    }

    /// Checks which rollback kind is available.
    pub async fn check_rollback(
        &self,
        kind: RollbackKind,
    ) -> Result<RollbackAvailability, ClientError> {
        let mut client = PreviewRollbackServiceClient::new(self.channel.clone());
        let response = self
            .unary(
                Operation::CheckRollback,
                client.check_rollback(Request::new(CheckRollbackRequest {
                    kind: protocol::rollback_kind(kind),
                })),
            )
            .await?
            .into_inner();
        Ok(protocol::decode_rollback_availability(response.kind))
    }
}
