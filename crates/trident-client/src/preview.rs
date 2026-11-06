use tonic::Request;

use trident_api::config::HostConfiguration;
use trident_proto::v1preview::{
    install_service_client::InstallServiceClient,
    rebuild_raid_service_client::RebuildRaidServiceClient,
    rollback_service_client::RollbackServiceClient as PreviewRollbackServiceClient,
    status_service_client::StatusServiceClient, validation_service_client::ValidationServiceClient,
    AbVolumeState, CheckRollbackKind, CheckRollbackRequest, FinalizeInstallRequest,
    GetActiveVolumeRequest, GetServicingStateRequest, InstallRequest, RebuildRaidRequest,
    ServicingState as ProtoServicingState, StageInstallRequest, ValidateHostConfigurationRequest,
};

use crate::{
    client::{proto_config, reboot_management, rollback_kind, serialize_config},
    operation::{decode_remote_error, invalid_protocol},
    ActiveVolume, Client, ClientError, ExecutionState, Operation, RebootPolicy,
    RollbackAvailability, RollbackKind, ServicingOperation, ServicingState, ValidationResult,
};

impl Client {
    /// Performs an install from a typed Host Configuration.
    pub async fn install(
        &self,
        config: &HostConfiguration,
        reboot: RebootPolicy,
    ) -> Result<ServicingOperation, ClientError> {
        self.install_yaml(serialize_config(Operation::Install, config)?, reboot)
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
                config: Some(proto_config(config)),
            }),
            finalize: Some(FinalizeInstallRequest {
                reboot: Some(reboot_management(reboot)),
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
        self.install_stage_yaml(serialize_config(Operation::InstallStage, config)?)
            .await
    }

    /// Stages an install using Host Configuration YAML unchanged.
    pub async fn install_stage_yaml(
        &self,
        config: impl Into<String>,
    ) -> Result<ServicingOperation, ClientError> {
        let request = StageInstallRequest {
            config: Some(proto_config(config)),
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
            reboot: Some(reboot_management(reboot)),
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
        match ProtoServicingState::try_from(response.state) {
            Ok(ProtoServicingState::Unspecified) => Ok(ServicingState::Unspecified),
            Ok(ProtoServicingState::NotProvisioned) => Ok(ServicingState::NotProvisioned),
            Ok(ProtoServicingState::InstallStaged) => Ok(ServicingState::CleanInstallStaged),
            Ok(ProtoServicingState::UpdateAbStaged) => Ok(ServicingState::AbUpdateStaged),
            Ok(ProtoServicingState::ManualRollbackAbStaged) => {
                Ok(ServicingState::ManualRollbackAbStaged)
            }
            Ok(ProtoServicingState::ManualRollbackRuntimeStaged) => {
                Ok(ServicingState::ManualRollbackRuntimeStaged)
            }
            Ok(ProtoServicingState::UpdateRuntimeStaged) => Ok(ServicingState::RuntimeUpdateStaged),
            Ok(ProtoServicingState::InstallFinalized) => Ok(ServicingState::CleanInstallFinalized),
            Ok(ProtoServicingState::UpdateAbFinalized) => Ok(ServicingState::AbUpdateFinalized),
            Ok(ProtoServicingState::ManualRollbackAbFinalized) => {
                Ok(ServicingState::ManualRollbackAbFinalized)
            }
            Ok(ProtoServicingState::Provisioned) => Ok(ServicingState::Provisioned),
            Ok(ProtoServicingState::UpdateAbHealthCheckFailed) => {
                Ok(ServicingState::AbUpdateHealthCheckFailed)
            }
            Err(_) => Ok(ServicingState::Unknown(response.state)),
        }
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
        match AbVolumeState::try_from(response.active_volume) {
            Ok(AbVolumeState::Unspecified) => Ok(ActiveVolume::Unspecified),
            Ok(AbVolumeState::NoVolume) => Ok(ActiveVolume::None),
            Ok(AbVolumeState::VolumeA) => Ok(ActiveVolume::VolumeA),
            Ok(AbVolumeState::VolumeB) => Ok(ActiveVolume::VolumeB),
            Err(_) => Ok(ActiveVolume::Unknown(response.active_volume)),
        }
    }

    /// Validates a typed Host Configuration.
    pub async fn validate_host_configuration(
        &self,
        config: &HostConfiguration,
    ) -> Result<ValidationResult, ClientError> {
        self.validate_host_configuration_yaml(serialize_config(
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
            config: Some(proto_config(config)),
        };
        let mut client = ValidationServiceClient::new(self.channel.clone());
        let response = self
            .unary(
                Operation::ValidateHostConfiguration,
                client.validate_host_configuration(Request::new(request)),
            )
            .await?
            .into_inner();
        let error = response.error.map(decode_remote_error);
        if response.ok == error.is_some() {
            return Err(invalid_protocol(
                Operation::ValidateHostConfiguration,
                if response.ok {
                    "successful validation carries an error"
                } else {
                    "failed validation is missing error details"
                },
                ExecutionState::Completed,
            ));
        }
        Ok(ValidationResult {
            valid: response.ok,
            error,
        })
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
                    kind: rollback_kind(kind),
                })),
            )
            .await?
            .into_inner();
        match CheckRollbackKind::try_from(response.kind) {
            Ok(CheckRollbackKind::Unspecified) => Ok(RollbackAvailability::Unspecified),
            Ok(CheckRollbackKind::NoRollbackAvailable) => Ok(RollbackAvailability::None),
            Ok(CheckRollbackKind::AbRollbackExpected) => Ok(RollbackAvailability::Ab),
            Ok(CheckRollbackKind::RuntimeRollbackExpected) => Ok(RollbackAvailability::Runtime),
            Err(_) => Ok(RollbackAvailability::Unknown(response.kind)),
        }
    }
}
