//! Implements the gRPC Services for the TridentServer struct.

use tonic::{async_trait, Request, Response, Status};

use trident_proto::v1::{version_service_server::VersionService, VersionRequest, VersionResponse};

use crate::{server::TridentServer, TRIDENT_VERSION};

#[async_trait]
impl VersionService for TridentServer {
    async fn version(
        &self,
        _request: Request<VersionRequest>,
    ) -> Result<Response<VersionResponse>, Status> {
        Ok(Response::new(VersionResponse {
            version: TRIDENT_VERSION.to_string(),
        }))
    }
}
