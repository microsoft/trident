use tonic::{async_trait, Request, Response, Status};

use trident_api::error::TridentResultExt;

use trident_proto::v1::{commit_service_server::CommitService, CommitRequest};

#[cfg(feature = "grpc-preview")]
use trident_api::error::{InternalError, TridentError};
#[cfg(feature = "grpc-preview")]
use trident_proto::v1preview::{
    commit_service_server::CommitService as CommitServicePreview, CheckRootRequest,
};

use crate::{
    server::{
        tridentserver::{datastore, ServicingResponseStream},
        TridentServer,
    },
    DataStore, Trident,
};

#[cfg(feature = "grpc-preview")]
use crate::server::tridentserver::RebootDecision;

#[async_trait]
impl CommitService for TridentServer {
    type CommitStream = ServicingResponseStream;
    async fn commit(
        &self,
        request: Request<CommitRequest>,
    ) -> Result<Response<Self::CommitStream>, Status> {
        let req = request.into_inner();

        let data_store_path = self.agent_config.datastore_path().to_owned();
        let logstream = self.logstream.clone();
        let tracestream = self.tracestream.clone();

        self.servicing_request("commit", super::reboot_allowed(&req.reboot), move || {
            let mut trident: Trident = Trident::new(None, &data_store_path, logstream, tracestream)
                .message("Failed to initialize Trident")?;

            let mut datastore =
                DataStore::open_or_create(&data_store_path).message("Failed to open datastore")?;

            let image_hash = datastore::stored_image_hash(&datastore);

            trident
                .commit(&mut datastore)
                .map(|(exit_kind, servicing_type)| {
                    (
                        exit_kind,
                        image_hash,
                        Some(super::servicing_type_to_kind(servicing_type)),
                    )
                })
        })
    }
}

#[cfg(feature = "grpc-preview")]
#[async_trait]
impl CommitServicePreview for TridentServer {
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
}
