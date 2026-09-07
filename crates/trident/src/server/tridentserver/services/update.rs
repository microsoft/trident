use tonic::{async_trait, Request, Response, Status};

use trident_api::{
    config::{HostConfigurationSource, Operation, Operations},
    error::TridentResultExt,
};
use trident_proto::v1::{
    update_service_server::UpdateService, FinalizeUpdateRequest, StageUpdateRequest, UpdateRequest,
};

use crate::{
    server::{
        tridentserver::{RebootDecision, ServicingResponseStream},
        TridentServer,
    },
    validation, DataStore, Trident,
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

        // Reject an unparsable Host Configuration payload before
        // servicing_request's correlation-ID pre-warm below, which for
        // this RPC creates the datastore as a side effect -- otherwise an
        // invalid payload would still leave a datastore behind, letting a
        // later request wrongly pass the "host not provisioned" existence
        // check. Parse-only (no semantic validate()), matching what
        // Trident::new does with this same string moments later.
        if let Err(e) = validation::parse_host_config(&host_config.config, None::<&std::path::Path>)
        {
            return Err(Status::invalid_argument(format!(
                "Invalid host configuration: {e:?}"
            )));
        }

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

                trident
                    .update(&mut datastore, Operations::all())
                    .map(|(k, h, st)| (k, h, Some(st.into())))
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

        // See the equivalent check in update() above for why this must
        // happen before servicing_request's datastore-creating pre-warm.
        if let Err(e) = validation::parse_host_config(&host_config.config, None::<&std::path::Path>)
        {
            return Err(Status::invalid_argument(format!(
                "Invalid host configuration: {e:?}"
            )));
        }

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

            trident
                .update(&mut datastore, Operation::Stage.into())
                .map(|(k, h, st)| (k, h, Some(st.into())))
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

                trident
                    .update(&mut datastore, Operation::Finalize.into())
                    .map(|(k, h, st)| (k, h, Some(st.into())))
            },
        )
    }
}
