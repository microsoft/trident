use tonic::{async_trait, Request, Response, Status};

use trident_api::{
    config::{HostConfigurationSource, Operation, Operations},
    error::TridentResultExt,
};
use trident_proto::v1preview::{
    update_service_server::UpdateService, FinalizeUpdateRequest, StageUpdateRequest, UpdateRequest,
};

use crate::{
    server::{
        tridentserver::{RebootDecision, ServicingResponseStream},
        TridentServer,
    },
    DataStore, Trident,
};

#[async_trait]
impl UpdateService for TridentServer {
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

        self.servicing_request(
            "update",
            super::reboot_allowed(&finalize.reboot),
            move || {
                let mut trident = Trident::new(
                    Some(HostConfigurationSource::RawString(host_config.config)),
                    &data_store_path,
                    logstream,
                    tracestream,
                )
                .message("Failed to initialize Trident")?;

                let mut datastore = DataStore::open_or_create(&data_store_path)
                    .message("Failed to open datastore")?;

                trident.update(&mut datastore, Operations::all())
            },
        )
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
            super::reboot_allowed(&finalize.reboot),
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
