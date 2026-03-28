//! Implements the InitializeService for the TridentServer.

use tonic::{async_trait, Request, Response, Status};

use trident_proto::v1::{
    initialize_service_server::InitializeService, OnlineInitializeRequest,
    OnlineInitializeResponse, StatusCode,
};

use crate::server::TridentServer;

#[async_trait]
impl InitializeService for TridentServer {
    async fn online_initialize(
        &self,
        _request: Request<OnlineInitializeRequest>,
    ) -> Result<Response<OnlineInitializeResponse>, Status> {
        Ok(Response::new(OnlineInitializeResponse {
            status: StatusCode::Success.into(),
            error: None,
        }))
    }
}
