//! Implements the gRPC TridentService for the TridentHarpoonServer struct.

use log::info;
use tonic::{async_trait, Request, Response, Status};

use harpoon::{
    trident_service_server::TridentService, CheckRootRequest, CommitRequest, FinalizeRequest,
    GetActiveVolumeRequest, GetActiveVolumeResponse, GetConfigRequest, GetConfigResponse,
    GetLastErrorRequest, GetLastErrorResponse, GetRequiredServicingTypeRequest,
    GetRequiredServicingTypeResponse, GetServicingStateRequest, GetServicingStateResponse,
    RebuildRaidRequest, ServicingRequest, StageRequest, StreamImageRequest,
    TridentError as HarpoonTridentError, ValidateHostConfigurationRequest,
    ValidateHostConfigurationResponse,
};
use trident_api::error::{InternalError, TridentError};

use crate::{
    server::{tridentserver::ServicingResponseStream, TridentHarpoonServer},
    validation,
};

/// Implements the gRPC TridentService for the TridentHarpoonServer struct.
#[async_trait]
impl TridentService for TridentHarpoonServer {
    // /// Sample data read method
    // ///
    // /// TODO: Remove once real methods are implemented.
    // async fn read_data(
    //     &self,
    //     _request: Request<ReadRequest>,
    // ) -> Result<Response<ReadResponse>, Status> {
    //     self.reading_request("read_data", || {
    //         let value = servicing::some_reading_operation("hello from server")?;
    //         Ok(ReadResponse { output: value })
    //     })
    // }

    // /// Sample servicing method
    // ///
    // /// TODO: Remove once real methods are implemented.
    // type DoProcessStream = ServicingResponseStream;
    // async fn do_process(
    //     &self,
    //     request: Request<DoProcessRequest>,
    // ) -> Result<Response<Self::DoProcessStream>, Status> {
    //     let process_req = request.into_inner();
    //     self.servicing_request("do_process", move || {
    //         servicing::some_servicing_operation(
    //             process_req.count,
    //             Duration::from_millis(process_req.interval_ms.into()),
    //         )
    //     })
    // }

    type InstallStream = ServicingResponseStream;
    async fn install(
        &self,
        _request: Request<ServicingRequest>,
    ) -> Result<Response<Self::InstallStream>, Status> {
        self.servicing_request("install", || {
            Err(TridentError::new(InternalError::Internal(
                "Not implemented: install",
            )))
        })
    }

    type InstallStageStream = ServicingResponseStream;
    async fn install_stage(
        &self,
        _request: Request<StageRequest>,
    ) -> Result<Response<Self::InstallStageStream>, Status> {
        self.servicing_request("install_stage", || {
            Err(TridentError::new(InternalError::Internal(
                "Not implemented: install_stage",
            )))
        })
    }

    type InstallFinalizeStream = ServicingResponseStream;
    async fn install_finalize(
        &self,
        _request: Request<FinalizeRequest>,
    ) -> Result<Response<Self::InstallFinalizeStream>, Status> {
        self.servicing_request("install_finalize", || {
            Err(TridentError::new(InternalError::Internal(
                "Not implemented: install_finalize",
            )))
        })
    }

    type UpdateStream = ServicingResponseStream;
    async fn update(
        &self,
        _request: Request<ServicingRequest>,
    ) -> Result<Response<Self::UpdateStream>, Status> {
        self.servicing_request("update", || {
            Err(TridentError::new(InternalError::Internal(
                "Not implemented: update",
            )))
        })
    }

    type UpdateStageStream = ServicingResponseStream;
    async fn update_stage(
        &self,
        _request: Request<StageRequest>,
    ) -> Result<Response<Self::UpdateStageStream>, Status> {
        self.servicing_request("update_stage", || {
            Err(TridentError::new(InternalError::Internal(
                "Not implemented: update_stage",
            )))
        })
    }

    type UpdateFinalizeStream = ServicingResponseStream;
    async fn update_finalize(
        &self,
        _request: Request<FinalizeRequest>,
    ) -> Result<Response<Self::UpdateFinalizeStream>, Status> {
        self.servicing_request("update_finalize", || {
            Err(TridentError::new(InternalError::Internal(
                "Not implemented: update_finalize",
            )))
        })
    }

    type CheckRootStream = ServicingResponseStream;
    async fn check_root(
        &self,
        _request: Request<CheckRootRequest>,
    ) -> Result<Response<Self::CheckRootStream>, Status> {
        self.servicing_request("check_root", || {
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
        self.servicing_request("commit", || {
            Err(TridentError::new(InternalError::Internal(
                "Not implemented: commit",
            )))
        })
    }

    type StreamImageStream = ServicingResponseStream;
    async fn stream_image(
        &self,
        _request: Request<StreamImageRequest>,
    ) -> Result<Response<Self::StreamImageStream>, Status> {
        self.servicing_request("stream_image", || {
            Err(TridentError::new(InternalError::Internal(
                "Not implemented: stream_image",
            )))
        })
    }

    type RebuildRaidStream = ServicingResponseStream;
    async fn rebuild_raid(
        &self,
        _request: Request<RebuildRaidRequest>,
    ) -> Result<Response<Self::RebuildRaidStream>, Status> {
        self.servicing_request("rebuild_raid", || {
            Err(TridentError::new(InternalError::Internal(
                "Not implemented: rebuild_raid",
            )))
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
        self.reading_request("get_active_volume", || {
            Err(TridentError::new(InternalError::Internal(
                "Not implemented: get_active_volume",
            )))
        })
        .await
    }
}
