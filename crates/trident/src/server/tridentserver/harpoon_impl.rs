//! Implements the gRPC TridentService for the TridentHarpoonServer struct.

use tonic::{async_trait, Request, Response, Status};
use url::Url;

use trident_api::{constants::IMAGE_CHECKSUM_IGNORED, error::TridentResultExt};
use trident_proto::v1::{
    streaming_service_server::StreamingService, version_service_server::VersionService,
    RebootHandling, RebootManagement, StreamDiskRequest, VersionRequest, VersionResponse,
};

use crate::{server::TridentHarpoonServer, DataStore, Trident, TRIDENT_VERSION};

use super::{RebootDecision, ServicingResponseStream};

// IMPORT BLOCKS FOR PREVIEW FEATURES

#[cfg(feature = "grpc-preview")]
use log::info;

#[cfg(feature = "grpc-preview")]
use trident_api::{
    config::{HostConfigurationSource, Operation, Operations},
    error::{InternalError, TridentError},
    status::AbVolumeSelection,
};
#[cfg(feature = "grpc-preview")]
use trident_proto::{
    v1::TridentError as HarpoonTridentError,
    v1preview::{
        commit_service_server::CommitService, install_service_server::InstallService,
        rebuild_raid_service_server::RebuildRaidService, rollback_service_server::RollbackService,
        status_service_server::StatusService, update_service_server::UpdateService,
        validation_service_server::ValidationService, AbVolumeState, CheckRollbackRequest,
        CheckRollbackResponse, CheckRootRequest, CommitRequest, FinalizeInstallRequest,
        FinalizeUpdateRequest, GetActiveVolumeRequest, GetActiveVolumeResponse, GetConfigRequest,
        GetConfigResponse, GetLastErrorRequest, GetLastErrorResponse,
        GetRequiredServicingTypeRequest, GetRequiredServicingTypeResponse, GetRollbackChainRequest,
        GetRollbackChainResponse, GetRollbackTargetRequest, GetRollbackTargetResponse,
        GetServicingStateRequest, GetServicingStateResponse, InstallRequest, RebuildRaidRequest,
        RollbackFinalizeRequest, RollbackRequest, RollbackStageRequest, StageInstallRequest,
        StageUpdateRequest, UpdateRequest, ValidateHostConfigurationRequest,
        ValidateHostConfigurationResponse,
    },
};

#[cfg(feature = "grpc-preview")]
use crate::{validation, ExitKind};

#[cfg(feature = "grpc-preview")]
use super::datastore;

/// Returns a `RebootDecision` indicating whether Trident can perform a reboot
/// given a provided optional RebootManagement struct.
fn reboot_allowed(reboot_opt: &Option<RebootManagement>) -> RebootDecision {
    if let Some(reboot) = reboot_opt {
        match reboot.handling() {
            // On unspecified, assume that Trident can handle the reboot, as
            // that is the safest option.
            RebootHandling::Unspecified => RebootDecision::Handle,

            // The caller explicitly specified that Trident can handle reboots,
            // so we allow it.
            RebootHandling::TridentHandlesReboot => RebootDecision::Handle,

            // The caller explicitly specified that they will handle reboots, so
            // we defer to them.
            RebootHandling::CallerHandlesReboot => RebootDecision::Defer,
        }
    } else {
        // If no reboot configuration is provided, we default to Trident
        // handling reboots.
        RebootDecision::Handle
    }
}

/// Implements the gRPC TridentService for the TridentHarpoonServer struct.
#[async_trait]
impl VersionService for TridentHarpoonServer {
    async fn version(
        &self,
        _request: Request<VersionRequest>,
    ) -> Result<Response<VersionResponse>, Status> {
        Ok(Response::new(VersionResponse {
            version: TRIDENT_VERSION.to_string(),
        }))
    }
}

#[async_trait]
impl StreamingService for TridentHarpoonServer {
    type StreamDiskStream = ServicingResponseStream;
    async fn stream_disk(
        &self,
        request: Request<StreamDiskRequest>,
    ) -> Result<Response<Self::StreamDiskStream>, Status> {
        let req = request.into_inner();

        // Parse the image URL from the request, returning an error if it is invalid.
        let url = Url::parse(&req.image_url).map_err(|e| {
            Status::invalid_argument(format!("Invalid image URL '{}': {}", req.image_url, e))
        })?;

        // If the image hash is not provided, we use the constant for ignored checksum.
        let image_hash = req
            .image_hash
            .clone()
            .unwrap_or_else(|| IMAGE_CHECKSUM_IGNORED.into());

        let data_store_path = self.agent_config.datastore_path().to_owned();
        let logstream = self.logstream.clone();
        let tracestream = self.tracestream.clone();

        self.servicing_request("stream_disk", reboot_allowed(&req.reboot), move || {
            let mut trident = Trident::new(None, &data_store_path, logstream, tracestream)
                .message("Failed to initialize Trident")?;

            let mut datastore =
                DataStore::open_or_create(&data_store_path).message("Failed to open datastore")?;

            trident.stream_image(&mut datastore, &url, &image_hash)
        })
    }
}

#[cfg(feature = "grpc-preview")]
#[async_trait]
impl InstallService for TridentHarpoonServer {
    type InstallStream = ServicingResponseStream;
    #[cfg(feature = "grpc-preview")]
    async fn install(
        &self,
        request: Request<InstallRequest>,
    ) -> Result<Response<Self::InstallStream>, Status> {
        let req = request.into_inner();
        let Some(staging) = req.stage else {
            return Err(Status::invalid_argument("Missing staging configuration"));
        };

        let Some(host_config) = staging.config else {
            return Err(Status::invalid_argument(
                "Missing host configuration in staging configuration",
            ));
        };

        let Some(finalize) = req.finalize else {
            return Err(Status::invalid_argument("Missing finalize configuration"));
        };

        let data_store_path = self.agent_config.datastore_path().to_owned();
        let logstream = self.logstream.clone();
        let tracestream = self.tracestream.clone();

        self.servicing_request("install", reboot_allowed(&finalize.reboot), move || {
            let mut trident = Trident::new(
                Some(HostConfigurationSource::RawString(host_config.config)),
                &data_store_path,
                logstream,
                tracestream,
            )
            .message("Failed to initialize Trident")?;

            let mut datastore =
                DataStore::open_or_create(&data_store_path).message("Failed to open datastore")?;

            trident.install(&mut datastore, Operations::all(), false, None)
        })
    }

    type InstallStageStream = ServicingResponseStream;
    async fn install_stage(
        &self,
        request: Request<StageInstallRequest>,
    ) -> Result<Response<Self::InstallStageStream>, Status> {
        let req = request.into_inner();

        let Some(host_config) = req.config else {
            return Err(Status::invalid_argument(
                "Missing host configuration in staging configuration",
            ));
        };

        let data_store_path = self.agent_config.datastore_path().to_owned();
        let logstream = self.logstream.clone();
        let tracestream = self.tracestream.clone();

        self.servicing_request("install_stage", RebootDecision::Error, move || {
            let mut trident = Trident::new(
                Some(HostConfigurationSource::RawString(host_config.config)),
                &data_store_path,
                logstream,
                tracestream,
            )
            .message("Failed to initialize Trident")?;

            let mut datastore =
                DataStore::open_or_create(&data_store_path).message("Failed to open datastore")?;

            trident.install(&mut datastore, Operation::Stage.into(), false, None)
        })
    }

    type InstallFinalizeStream = ServicingResponseStream;
    async fn install_finalize(
        &self,
        request: Request<FinalizeInstallRequest>,
    ) -> Result<Response<Self::InstallFinalizeStream>, Status> {
        let finalize = request.into_inner();

        let data_store_path = self.agent_config.datastore_path().to_owned();
        let logstream = self.logstream.clone();
        let tracestream = self.tracestream.clone();

        self.servicing_request(
            "install_finalize",
            reboot_allowed(&finalize.reboot),
            move || {
                let mut trident = Trident::new(None, &data_store_path, logstream, tracestream)
                    .message("Failed to initialize Trident")?;

                let mut datastore = DataStore::open_or_create(&data_store_path)
                    .message("Failed to open datastore")?;

                trident.install(&mut datastore, Operation::Finalize.into(), false, None)
            },
        )
    }
}

#[cfg(feature = "grpc-preview")]
#[async_trait]
impl UpdateService for TridentHarpoonServer {
    type UpdateStream = ServicingResponseStream;
    async fn update(
        &self,
        request: Request<UpdateRequest>,
    ) -> Result<Response<Self::UpdateStream>, Status> {
        let req = request.into_inner();
        let Some(staging) = req.stage else {
            return Err(Status::invalid_argument("Missing staging configuration"));
        };

        let Some(host_config) = staging.config else {
            return Err(Status::invalid_argument(
                "Missing host configuration in staging configuration",
            ));
        };

        let Some(finalize) = req.finalize else {
            return Err(Status::invalid_argument("Missing finalize configuration"));
        };

        let data_store_path = self.agent_config.datastore_path().to_owned();
        let logstream = self.logstream.clone();
        let tracestream = self.tracestream.clone();

        self.servicing_request("update", reboot_allowed(&finalize.reboot), move || {
            let mut trident = Trident::new(
                Some(HostConfigurationSource::RawString(host_config.config)),
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
        request: Request<StageUpdateRequest>,
    ) -> Result<Response<Self::UpdateStageStream>, Status> {
        let req = request.into_inner();

        let Some(host_config) = req.config else {
            return Err(Status::invalid_argument(
                "Missing host configuration in staging configuration",
            ));
        };

        let data_store_path = self.agent_config.datastore_path().to_owned();
        let logstream = self.logstream.clone();
        let tracestream = self.tracestream.clone();

        self.servicing_request("update_stage", RebootDecision::Error, move || {
            let mut trident = Trident::new(
                Some(HostConfigurationSource::RawString(host_config.config)),
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
        request: Request<FinalizeUpdateRequest>,
    ) -> Result<Response<Self::UpdateFinalizeStream>, Status> {
        let finalize = request.into_inner();

        let data_store_path = self.agent_config.datastore_path().to_owned();
        let logstream = self.logstream.clone();
        let tracestream = self.tracestream.clone();

        self.servicing_request(
            "update_finalize",
            reboot_allowed(&finalize.reboot),
            move || {
                let mut trident = Trident::new(None, &data_store_path, logstream, tracestream)
                    .message("Failed to initialize Trident")?;

                let mut datastore = DataStore::open_or_create(&data_store_path)
                    .message("Failed to open datastore")?;

                trident.update(&mut datastore, Operation::Finalize.into())
            },
        )
    }
}

#[cfg(feature = "grpc-preview")]
#[async_trait]
impl CommitService for TridentHarpoonServer {
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

    #[cfg(feature = "grpc-preview")]
    type CommitStream = ServicingResponseStream;
    #[cfg(feature = "grpc-preview")]
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
}

#[cfg(feature = "grpc-preview")]
#[async_trait]
impl ValidationService for TridentHarpoonServer {
    async fn validate_host_configuration(
        &self,
        request: Request<ValidateHostConfigurationRequest>,
    ) -> Result<Response<ValidateHostConfigurationResponse>, Status> {
        // Validate is different because it only acts upon the input and does
        // not read or modify state in any way, so we are free to run this
        // whenever without doing any lock checks.
        info!("Received Host Configuration validation request");
        let Some(host_config) = request.into_inner().config else {
            return Err(Status::invalid_argument(
                "Missing host configuration in staging configuration",
            ));
        };

        let error = validation::validate_host_config_string(&host_config.config)
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
}

#[cfg(feature = "grpc-preview")]
#[async_trait]
impl StatusService for TridentHarpoonServer {
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
        let data_store_path = self.agent_config.datastore_path().to_owned();
        self.reading_request("get_servicing_state", move || {
            let datastore =
                DataStore::open(&data_store_path).message("Failed to open datastore")?;

            Ok(GetServicingStateResponse {
                state: datastore::servicing_state_from_datastore(&datastore).into(),
            })
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
}

#[cfg(feature = "grpc-preview")]
#[async_trait]
impl RollbackService for TridentHarpoonServer {
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
    ) -> Result<Response<GetRollbackChainResponse>, Status> {
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
    ) -> Result<Response<GetRollbackTargetResponse>, Status> {
        self.reading_request("get_rollback_target", || {
            Err(TridentError::new(InternalError::Internal(
                "Not implemented: get_rollback_target",
            )))
        })
        .await
    }
}

#[cfg(feature = "grpc-preview")]
#[async_trait]
impl RebuildRaidService for TridentHarpoonServer {
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
}
