//! Implements the gRPC TridentService for the TridentHarpoonServer struct.

use log::info;
use tonic::{async_trait, Request, Response, Status};

use harpoon::{
    trident_service_server::TridentService, AbVolumeState, CheckRollbackRequest,
    CheckRollbackResponse, CheckRootRequest, CommitRequest, FinalizeRequest,
    GetActiveVolumeRequest, GetActiveVolumeResponse, GetConfigRequest, GetConfigResponse,
    GetLastErrorRequest, GetLastErrorResponse, GetRequiredServicingTypeRequest,
    GetRequiredServicingTypeResponse, GetRollbackChainRequest, GetRollbackTargetRequest,
    GetServicingStateRequest, GetServicingStateResponse, RebuildRaidRequest,
    RollbackFinalizeRequest, RollbackRequest, RollbackStageRequest, ServicingRequest, StageRequest,
    StreamImageRequest, TridentError as HarpoonTridentError, ValidateHostConfigurationRequest,
    ValidateHostConfigurationResponse, VersionRequest, VersionResponse,
};
use trident_api::{
    config::{HostConfigurationSource, Operation, Operations},
    error::{InternalError, TridentError, TridentResultExt},
    status::AbVolumeSelection,
};
use url::Url;

use crate::{
    server::TridentHarpoonServer, stream, validation, DataStore, ExitKind, Trident, TRIDENT_VERSION,
};

use super::{RebootDecision, ServicingResponseStream};

/// Returns a `RebootDecision` indicating whether Trident can perform a reboot
/// given a provided FinalizeRequest.
fn reboot_allowed(finalize: &FinalizeRequest) -> RebootDecision {
    // If the finalize request indicates that the orchestrator handles reboots,
    // then Trident should NOT perform a reboot itself.
    if finalize.orchestrator_handles_reboot {
        RebootDecision::Defer
    } else {
        RebootDecision::Handle
    }
}

/// Implements the gRPC TridentService for the TridentHarpoonServer struct.
#[async_trait]
impl TridentService for TridentHarpoonServer {
    async fn version(
        &self,
        _request: Request<VersionRequest>,
    ) -> Result<Response<VersionResponse>, Status> {
        Ok(Response::new(VersionResponse {
            version: TRIDENT_VERSION.to_string(),
        }))
    }

    type InstallStream = ServicingResponseStream;
    async fn install(
        &self,
        request: Request<ServicingRequest>,
    ) -> Result<Response<Self::InstallStream>, Status> {
        let req = request.into_inner();
        let Some(staging) = req.stage else {
            return Err(Status::invalid_argument("Missing staging configuration"));
        };
        let Some(finalize) = req.finalize else {
            return Err(Status::invalid_argument("Missing finalize configuration"));
        };

        let data_store_path = self.agent_config.datastore_path().to_owned();
        let logstream = self.logstream.clone();
        let tracestream = self.tracestream.clone();

        self.servicing_request("install", reboot_allowed(&finalize), move || {
            let mut trident = Trident::new(
                Some(HostConfigurationSource::RawString(staging.config)),
                &data_store_path,
                logstream,
                tracestream,
            )
            .message("Failed to initialize Trident")?;

            let mut datastore =
                DataStore::open_or_create(&data_store_path).message("Failed to open datastore")?;

            trident.install(&mut datastore, Operations::all(), false)
        })
    }

    type InstallStageStream = ServicingResponseStream;
    async fn install_stage(
        &self,
        request: Request<StageRequest>,
    ) -> Result<Response<Self::InstallStageStream>, Status> {
        let req = request.into_inner();

        let data_store_path = self.agent_config.datastore_path().to_owned();
        let logstream = self.logstream.clone();
        let tracestream = self.tracestream.clone();

        self.servicing_request("install_stage", RebootDecision::Error, move || {
            let mut trident = Trident::new(
                Some(HostConfigurationSource::RawString(req.config)),
                &data_store_path,
                logstream,
                tracestream,
            )
            .message("Failed to initialize Trident")?;

            let mut datastore =
                DataStore::open_or_create(&data_store_path).message("Failed to open datastore")?;

            trident.install(&mut datastore, Operation::Stage.into(), false)
        })
    }

    type InstallFinalizeStream = ServicingResponseStream;
    async fn install_finalize(
        &self,
        request: Request<FinalizeRequest>,
    ) -> Result<Response<Self::InstallFinalizeStream>, Status> {
        let finalize = request.into_inner();

        let data_store_path = self.agent_config.datastore_path().to_owned();
        let logstream = self.logstream.clone();
        let tracestream = self.tracestream.clone();

        self.servicing_request("install_finalize", reboot_allowed(&finalize), move || {
            let mut trident = Trident::new(None, &data_store_path, logstream, tracestream)
                .message("Failed to initialize Trident")?;

            let mut datastore =
                DataStore::open_or_create(&data_store_path).message("Failed to open datastore")?;

            trident.install(&mut datastore, Operation::Finalize.into(), false)
        })
    }

    type UpdateStream = ServicingResponseStream;
    async fn update(
        &self,
        request: Request<ServicingRequest>,
    ) -> Result<Response<Self::UpdateStream>, Status> {
        let req = request.into_inner();
        let Some(staging) = req.stage else {
            return Err(Status::invalid_argument("Missing staging configuration"));
        };
        let Some(finalize) = req.finalize else {
            return Err(Status::invalid_argument("Missing finalize configuration"));
        };

        let data_store_path = self.agent_config.datastore_path().to_owned();
        let logstream = self.logstream.clone();
        let tracestream = self.tracestream.clone();

        self.servicing_request("update", reboot_allowed(&finalize), move || {
            let mut trident = Trident::new(
                Some(HostConfigurationSource::RawString(staging.config)),
                &data_store_path,
                logstream,
                tracestream,
            )
            .message("Failed to initialize Trident")?;

            let mut datastore =
                DataStore::open_or_create(&data_store_path).message("Failed to open datastore")?;

            trident.update(&mut datastore, Operations::all())
        })
    }

    type UpdateStageStream = ServicingResponseStream;
    async fn update_stage(
        &self,
        request: Request<StageRequest>,
    ) -> Result<Response<Self::UpdateStageStream>, Status> {
        let req = request.into_inner();

        let data_store_path = self.agent_config.datastore_path().to_owned();
        let logstream = self.logstream.clone();
        let tracestream = self.tracestream.clone();

        self.servicing_request("update_stage", RebootDecision::Error, move || {
            let mut trident = Trident::new(
                Some(HostConfigurationSource::RawString(req.config)),
                &data_store_path,
                logstream,
                tracestream,
            )
            .message("Failed to initialize Trident")?;

            let mut datastore =
                DataStore::open_or_create(&data_store_path).message("Failed to open datastore")?;

            trident.update(&mut datastore, Operation::Stage.into())
        })
    }

    type UpdateFinalizeStream = ServicingResponseStream;
    async fn update_finalize(
        &self,
        request: Request<FinalizeRequest>,
    ) -> Result<Response<Self::UpdateFinalizeStream>, Status> {
        let finalize = request.into_inner();

        let data_store_path = self.agent_config.datastore_path().to_owned();
        let logstream = self.logstream.clone();
        let tracestream = self.tracestream.clone();

        self.servicing_request("update_finalize", reboot_allowed(&finalize), move || {
            let mut trident = Trident::new(None, &data_store_path, logstream, tracestream)
                .message("Failed to initialize Trident")?;

            let mut datastore =
                DataStore::open_or_create(&data_store_path).message("Failed to open datastore")?;

            trident.update(&mut datastore, Operation::Finalize.into())
        })
    }

    type CheckRootStream = ServicingResponseStream;
    async fn check_root(
        &self,
        _request: Request<CheckRootRequest>,
    ) -> Result<Response<Self::CheckRootStream>, Status> {
        self.servicing_request("check_root", RebootDecision::Error, || {
            Err(TridentError::new(InternalError::Internal(
                "Not implemented: check_root",
            )))
        })
    }

    type CommitStream = ServicingResponseStream;
    async fn commit(
        &self,
        _request: Request<CommitRequest>,
    ) -> Result<Response<Self::CommitStream>, Status> {
        let data_store_path = self.agent_config.datastore_path().to_owned();
        let logstream = self.logstream.clone();
        let tracestream = self.tracestream.clone();

        self.servicing_request("commit", RebootDecision::Error, move || {
            let mut trident = Trident::new(None, &data_store_path, logstream, tracestream)
                .message("Failed to initialize Trident")?;

            let mut datastore =
                DataStore::open_or_create(&data_store_path).message("Failed to open datastore")?;

            trident.commit(&mut datastore)
        })
    }

    type StreamImageStream = ServicingResponseStream;
    async fn stream_image(
        &self,
        request: Request<StreamImageRequest>,
    ) -> Result<Response<Self::StreamImageStream>, Status> {
        let req = request.into_inner();

        let url = Url::parse(&req.image_path).map_err(|e| {
            Status::invalid_argument(format!("Invalid image URL '{}': {}", req.image_path, e))
        })?;

        let Some(finalize) = req.finalize else {
            return Err(Status::invalid_argument("Missing finalize configuration"));
        };

        let data_store_path = self.agent_config.datastore_path().to_owned();
        let logstream = self.logstream.clone();
        let tracestream = self.tracestream.clone();

        self.servicing_request("stream_image", reboot_allowed(&finalize), move || {
            let config = stream::config_from_image_url(url, &req.image_hash)
                .message("Failed to generate Host Configuration from image URL")?;

            let mut trident = Trident::new(
                Some(HostConfigurationSource::Embedded(Box::new(config))),
                &data_store_path,
                logstream,
                tracestream,
            )
            .message("Failed to initialize Trident")?;

            let mut datastore =
                DataStore::open_or_create(&data_store_path).message("Failed to open datastore")?;

            trident.install(&mut datastore, Operations::all(), false)
        })
    }

    type RebuildRaidStream = ServicingResponseStream;
    async fn rebuild_raid(
        &self,
        _request: Request<RebuildRaidRequest>,
    ) -> Result<Response<Self::RebuildRaidStream>, Status> {
        let data_store_path = self.agent_config.datastore_path().to_owned();
        let logstream = self.logstream.clone();
        let tracestream = self.tracestream.clone();

        self.servicing_request("rebuild_raid", RebootDecision::Error, move || {
            let mut trident = Trident::new(None, &data_store_path, logstream, tracestream)
                .message("Failed to initialize Trident")?;

            let mut datastore =
                DataStore::open_or_create(&data_store_path).message("Failed to open datastore")?;

            trident
                .rebuild_raid(&mut datastore)
                .message("Failed to rebuild RAID arrays")?;

            Ok(ExitKind::Done)
        })
    }

    async fn validate_host_configuration(
        &self,
        request: Request<ValidateHostConfigurationRequest>,
    ) -> Result<Response<ValidateHostConfigurationResponse>, Status> {
        // Validate is different because it only acts upon the input and does
        // not read or modify state in any way, so we are free to run this
        // whenever without doing any lock checks.
        info!("Received Host Configuration validation request");
        let error = validation::validate_host_config_string(&request.into_inner().config)
            .err()
            .map(HarpoonTridentError::from);
        Ok(Response::new(ValidateHostConfigurationResponse {
            ok: error.is_none(),
            error,
        }))
    }

    async fn get_required_servicing_type(
        &self,
        _request: Request<GetRequiredServicingTypeRequest>,
    ) -> Result<Response<GetRequiredServicingTypeResponse>, Status> {
        self.reading_request("get_required_servicing_type", || {
            Err(TridentError::new(InternalError::Internal(
                "Not implemented: get_required_servicing_type",
            )))
        })
        .await
    }

    async fn get_provisioned_config(
        &self,
        _request: Request<GetConfigRequest>,
    ) -> Result<Response<GetConfigResponse>, Status> {
        self.reading_request("get_provisioned_config", || {
            Err(TridentError::new(InternalError::Internal(
                "Not implemented: get_provisioned_config",
            )))
        })
        .await
    }

    async fn get_servicing_config(
        &self,
        _request: Request<GetConfigRequest>,
    ) -> Result<Response<GetConfigResponse>, Status> {
        self.reading_request("get_servicing_config", || {
            Err(TridentError::new(InternalError::Internal(
                "Not implemented: get_servicing_config",
            )))
        })
        .await
    }

    async fn get_last_error(
        &self,
        _request: Request<GetLastErrorRequest>,
    ) -> Result<Response<GetLastErrorResponse>, Status> {
        self.reading_request("get_last_error", || {
            Err(TridentError::new(InternalError::Internal(
                "Not implemented: get_last_error",
            )))
        })
        .await
    }

    async fn get_servicing_state(
        &self,
        _request: Request<GetServicingStateRequest>,
    ) -> Result<Response<GetServicingStateResponse>, Status> {
        self.reading_request("get_servicing_state", || {
            Err(TridentError::new(InternalError::Internal(
                "Not implemented: get_servicing_state",
            )))
        })
        .await
    }

    async fn get_active_volume(
        &self,
        _request: Request<GetActiveVolumeRequest>,
    ) -> Result<Response<GetActiveVolumeResponse>, Status> {
        let data_store_path = self.agent_config.datastore_path().to_owned();
        self.reading_request("get_active_volume", move || {
            let datastore =
                DataStore::open(&data_store_path).message("Failed to open datastore")?;

            Ok(GetActiveVolumeResponse {
                active_volume: match datastore.host_status().ab_active_volume.as_ref() {
                    Some(AbVolumeSelection::VolumeA) => AbVolumeState::VolumeA,
                    Some(AbVolumeSelection::VolumeB) => AbVolumeState::VolumeB,
                    None => AbVolumeState::NoVolume,
                }
                .into(),
            })
        })
        .await
    }

    async fn check_rollback(
        &self,
        _request: Request<CheckRollbackRequest>,
    ) -> Result<Response<CheckRollbackResponse>, Status> {
        self.reading_request("check_rollback", || {
            Err(TridentError::new(InternalError::Internal(
                "Not implemented: check_rollback",
            )))
        })
        .await
    }

    type RollbackStream = ServicingResponseStream;
    async fn rollback(
        &self,
        _request: Request<RollbackRequest>,
    ) -> Result<Response<Self::RollbackStream>, Status> {
        self.servicing_request("rollback", RebootDecision::Error, || {
            Err(TridentError::new(InternalError::Internal(
                "Not implemented: rollback",
            )))
        })
    }

    type RollbackStageStream = ServicingResponseStream;
    async fn rollback_stage(
        &self,
        _request: Request<RollbackStageRequest>,
    ) -> Result<Response<Self::RollbackStageStream>, Status> {
        self.servicing_request("rollback_stage", RebootDecision::Error, || {
            Err(TridentError::new(InternalError::Internal(
                "Not implemented: rollback_stage",
            )))
        })
    }

    type RollbackFinalizeStream = ServicingResponseStream;
    async fn rollback_finalize(
        &self,
        _request: Request<RollbackFinalizeRequest>,
    ) -> Result<Response<Self::RollbackFinalizeStream>, Status> {
        self.servicing_request("rollback_finalize", RebootDecision::Error, || {
            Err(TridentError::new(InternalError::Internal(
                "Not implemented: rollback_finalize",
            )))
        })
    }

    async fn get_rollback_chain(
        &self,
        _request: Request<GetRollbackChainRequest>,
    ) -> Result<Response<harpoon::GetRollbackChainResponse>, Status> {
        self.reading_request("get_rollback_chain", || {
            Err(TridentError::new(InternalError::Internal(
                "Not implemented: get_rollback_chain",
            )))
        })
        .await
    }

    async fn get_rollback_target(
        &self,
        _request: Request<GetRollbackTargetRequest>,
    ) -> Result<Response<harpoon::GetRollbackTargetResponse>, Status> {
        self.reading_request("get_rollback_target", || {
            Err(TridentError::new(InternalError::Internal(
                "Not implemented: get_rollback_target",
            )))
        })
        .await
    }
}
