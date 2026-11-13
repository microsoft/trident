use tonic::{async_trait, Request, Response, Status};

use trident_api::{
    error::{InternalError, TridentError, TridentResultExt},
    status::AbVolumeSelection,
};
use trident_proto::v1preview::{
    status_service_server::StatusService, AbVolumeState, GetActiveVolumeRequest,
    GetActiveVolumeResponse, GetConfigRequest, GetConfigResponse, GetLastErrorRequest,
    GetLastErrorResponse, GetServicingStateRequest, GetServicingStateResponse,
};

use crate::{
    server::{tridentserver::datastore, TridentServer},
    DataStore,
};

#[async_trait]
impl StatusService for TridentServer {
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
