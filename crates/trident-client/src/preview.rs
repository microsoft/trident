use tonic::Request;

use trident_api::{
    config::HostConfiguration,
    status::{AbVolumeSelection, ServicingState, ServicingType},
};
use trident_proto::v1preview::{
    commit_service_client::CommitServiceClient as PreviewCommitServiceClient,
    install_service_client::InstallServiceClient,
    rebuild_raid_service_client::RebuildRaidServiceClient,
    rollback_service_client::RollbackServiceClient as PreviewRollbackServiceClient,
    status_service_client::StatusServiceClient, validation_service_client::ValidationServiceClient,
    AbVolumeState, CheckRollbackKind, CheckRollbackRequest, CheckRootRequest,
    FinalizeInstallRequest, GetActiveVolumeRequest, GetConfigRequest, GetLastErrorRequest,
    GetRequiredServicingTypeRequest, GetRollbackChainRequest, GetRollbackTargetRequest,
    GetServicingStateRequest, InstallRequest, RebuildRaidRequest,
    ServicingState as ProtoServicingState, ServicingType as ProtoServicingType,
    StageInstallRequest, ValidateHostConfigurationRequest,
};

use crate::{
    client::{proto_config, reboot_management, rollback_kind, serialize_config},
    operation::{decode_remote_error, invalid_protocol},
    Client, ClientError, ExecutionState, Operation, RebootPolicy, RemoteError,
    RollbackAvailability, RollbackKind, ServicingOperation, ValidationResult,
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

    /// Returns the currently provisioned Host Configuration.
    pub async fn provisioned_config(&self) -> Result<Option<HostConfiguration>, ClientError> {
        self.provisioned_config_yaml()
            .await?
            .map(|yaml| decode_config(Operation::ProvisionedConfig, &yaml))
            .transpose()
    }

    /// Returns the currently provisioned Host Configuration YAML unchanged.
    pub async fn provisioned_config_yaml(&self) -> Result<Option<String>, ClientError> {
        let mut client = StatusServiceClient::new(self.channel.clone());
        let response = self
            .unary(
                Operation::ProvisionedConfig,
                client.get_provisioned_config(Request::new(GetConfigRequest {})),
            )
            .await?
            .into_inner();
        Ok(response.config.map(|config| config.config))
    }

    /// Returns the active servicing Host Configuration.
    pub async fn servicing_config(&self) -> Result<Option<HostConfiguration>, ClientError> {
        self.servicing_config_yaml()
            .await?
            .map(|yaml| decode_config(Operation::ServicingConfig, &yaml))
            .transpose()
    }

    /// Returns the active servicing Host Configuration YAML unchanged.
    pub async fn servicing_config_yaml(&self) -> Result<Option<String>, ClientError> {
        let mut client = StatusServiceClient::new(self.channel.clone());
        let response = self
            .unary(
                Operation::ServicingConfig,
                client.get_servicing_config(Request::new(GetConfigRequest {})),
            )
            .await?
            .into_inner();
        Ok(response.config.map(|config| config.config))
    }

    /// Returns the most recently recorded servicing error.
    pub async fn last_error(&self) -> Result<Option<RemoteError>, ClientError> {
        let mut client = StatusServiceClient::new(self.channel.clone());
        let response = self
            .unary(
                Operation::LastError,
                client.get_last_error(Request::new(GetLastErrorRequest {})),
            )
            .await?
            .into_inner();
        response
            .error
            .map(|error| decode_remote_error(Operation::LastError, error))
            .transpose()
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
            Ok(ProtoServicingState::Unspecified) => Err(invalid_protocol(
                Operation::ServicingState,
                "servicing state is unspecified",
                ExecutionState::Completed,
            )),
            Err(_) => Err(invalid_protocol(
                Operation::ServicingState,
                format!("servicing state {} is invalid", response.state),
                ExecutionState::Completed,
            )),
        }
    }

    /// Returns the active A/B volume, or `None` when the host has no active volume.
    pub async fn active_volume(&self) -> Result<Option<AbVolumeSelection>, ClientError> {
        let mut client = StatusServiceClient::new(self.channel.clone());
        let response = self
            .unary(
                Operation::ActiveVolume,
                client.get_active_volume(Request::new(GetActiveVolumeRequest {})),
            )
            .await?
            .into_inner();
        match AbVolumeState::try_from(response.active_volume) {
            Ok(AbVolumeState::NoVolume) => Ok(None),
            Ok(AbVolumeState::VolumeA) => Ok(Some(AbVolumeSelection::VolumeA)),
            Ok(AbVolumeState::VolumeB) => Ok(Some(AbVolumeSelection::VolumeB)),
            Ok(AbVolumeState::Unspecified) => Err(invalid_protocol(
                Operation::ActiveVolume,
                "active volume is unspecified",
                ExecutionState::Completed,
            )),
            Err(_) => Err(invalid_protocol(
                Operation::ActiveVolume,
                format!("active volume {} is invalid", response.active_volume),
                ExecutionState::Completed,
            )),
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
        let error = response
            .error
            .map(|error| decode_remote_error(Operation::ValidateHostConfiguration, error))
            .transpose()?;
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

    /// Determines servicing required for a typed Host Configuration.
    pub async fn required_servicing_type(
        &self,
        config: &HostConfiguration,
    ) -> Result<ServicingType, ClientError> {
        self.required_servicing_type_yaml(serialize_config(
            Operation::RequiredServicingType,
            config,
        )?)
        .await
    }

    /// Determines servicing required for Host Configuration YAML unchanged.
    pub async fn required_servicing_type_yaml(
        &self,
        config: impl Into<String>,
    ) -> Result<ServicingType, ClientError> {
        let request = GetRequiredServicingTypeRequest {
            config: Some(proto_config(config)),
        };
        let mut client = ValidationServiceClient::new(self.channel.clone());
        let response = self
            .unary(
                Operation::RequiredServicingType,
                client.get_required_servicing_type(Request::new(request)),
            )
            .await?
            .into_inner();
        match ProtoServicingType::try_from(response.servicing_type) {
            Ok(ProtoServicingType::NoActiveServicing) => Ok(ServicingType::NoActiveServicing),
            Ok(ProtoServicingType::RuntimeUpdate) => Ok(ServicingType::RuntimeUpdate),
            Ok(ProtoServicingType::AbUpdate) => Ok(ServicingType::AbUpdate),
            Ok(ProtoServicingType::CleanInstall) => Ok(ServicingType::CleanInstall),
            Ok(ProtoServicingType::Unspecified) => Err(invalid_protocol(
                Operation::RequiredServicingType,
                "required servicing type is unspecified",
                ExecutionState::Completed,
            )),
            Err(_) => Err(invalid_protocol(
                Operation::RequiredServicingType,
                format!(
                    "required servicing type {} is invalid",
                    response.servicing_type
                ),
                ExecutionState::Completed,
            )),
        }
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
            Ok(CheckRollbackKind::NoRollbackAvailable) => Ok(RollbackAvailability::None),
            Ok(CheckRollbackKind::AbRollbackExpected) => Ok(RollbackAvailability::Ab),
            Ok(CheckRollbackKind::RuntimeRollbackExpected) => Ok(RollbackAvailability::Runtime),
            Ok(CheckRollbackKind::Unspecified) => Err(invalid_protocol(
                Operation::CheckRollback,
                "rollback availability is unspecified",
                ExecutionState::Completed,
            )),
            Err(_) => Err(invalid_protocol(
                Operation::CheckRollback,
                format!("rollback availability {} is invalid", response.kind),
                ExecutionState::Completed,
            )),
        }
    }

    /// Returns the available rollback chain.
    pub async fn rollback_chain(&self) -> Result<Vec<String>, ClientError> {
        let mut client = PreviewRollbackServiceClient::new(self.channel.clone());
        let response = self
            .unary(
                Operation::RollbackChain,
                client.get_rollback_chain(Request::new(GetRollbackChainRequest {})),
            )
            .await?
            .into_inner();
        Ok(response.rollback_chain)
    }

    /// Returns the next rollback target Host Configuration.
    pub async fn rollback_target(&self) -> Result<Option<HostConfiguration>, ClientError> {
        self.rollback_target_yaml()
            .await?
            .map(|yaml| decode_config(Operation::RollbackTarget, &yaml))
            .transpose()
    }

    /// Returns the next rollback target Host Configuration YAML unchanged.
    pub async fn rollback_target_yaml(&self) -> Result<Option<String>, ClientError> {
        let mut client = PreviewRollbackServiceClient::new(self.channel.clone());
        let response = self
            .unary(
                Operation::RollbackTarget,
                client.get_rollback_target(Request::new(GetRollbackTargetRequest {})),
            )
            .await?
            .into_inner();
        Ok(response.rollback_target)
    }

    /// Checks that the host booted from the expected root filesystem.
    pub async fn check_root(&self) -> Result<ServicingOperation, ClientError> {
        let mut client = PreviewCommitServiceClient::new(self.channel.clone());
        self.servicing(
            Operation::CheckRoot,
            client.check_root(Request::new(CheckRootRequest {})),
        )
        .await
    }
}

fn decode_config(operation: Operation, yaml: &str) -> Result<HostConfiguration, ClientError> {
    serde_yaml::from_str(yaml).map_err(|error| ClientError::Deserialization {
        operation,
        details: error.to_string(),
    })
}
