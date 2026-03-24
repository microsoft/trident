use tonic::{async_trait, Request, Response, Status};

use trident_api::error::{InternalError, TridentError};
use trident_proto::v1preview::{
    rollback_service_server::RollbackService, CheckRollbackRequest, CheckRollbackResponse,
    GetRollbackChainRequest, GetRollbackChainResponse, GetRollbackTargetRequest,
    GetRollbackTargetResponse, RollbackFinalizeRequest, RollbackRequest, RollbackStageRequest,
};

use crate::server::{
    tridentserver::{RebootDecision, ServicingResponseStream},
    TridentServer,
};

#[async_trait]
impl RollbackService for TridentServer {
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
