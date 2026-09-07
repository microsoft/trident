use tonic::{async_trait, Request, Response, Status};

use trident_api::{
    config::{HostConfigurationSource, Operation, Operations},
    error::{InvalidInputError, TridentError, TridentResultExt},
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
            return Err(self.reject_invalid_argument(
                "update",
                "stage",
                "Missing staging configuration",
            ));
        };

        let Some(host_config) = staging.config else {
            return Err(self.reject_invalid_argument(
                "update",
                "stage.config",
                "Missing host configuration in staging configuration",
            ));
        };

        let Some(finalize) = req.finalize else {
            return Err(self.reject_invalid_argument(
                "update",
                "finalize",
                "Missing finalize configuration",
            ));
        };

        // Reject an unparsable Host Configuration payload before
        // servicing_request's installation-ID pre-warm below, which for
        // this RPC creates the datastore as a side effect -- otherwise an
        // invalid payload would still leave a datastore behind, letting a
        // later request wrongly pass the "host not provisioned" existence
        // check. Parse-only (no semantic validate()), matching what
        // Trident::new does with this same string moments later.
        // (`reject_invalid_config` itself never creates a datastore --
        // see `TridentServer::refresh_installation_id_readonly` -- so
        // this ordering also avoids ever needing to clean one up on the
        // rejected path.)
        if let Err(e) = validation::parse_host_config(&host_config.config, None::<&std::path::Path>)
        {
            let message = format!("Invalid host configuration: {e:?}");
            return Err(self.reject_invalid_config("update", e, message));
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
            return Err(self.reject_invalid_argument(
                "update_stage",
                "config",
                "Missing host configuration in staging configuration",
            ));
        };

        // See the equivalent check in update() above for why this must
        // happen before servicing_request's datastore-creating pre-warm.
        if let Err(e) = validation::parse_host_config(&host_config.config, None::<&std::path::Path>)
        {
            let message = format!("Invalid host configuration: {e:?}");
            return Err(self.reject_invalid_config("update_stage", e, message));
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
                // Finalize-only: cannot itself stage anything, so it must
                // never create a datastore on an unprovisioned host (see
                // `DataStore::may_initialize_datastore_for_command`).
                // `Trident::new`'s own `open_or_create` below would
                // otherwise silently create one for a request that
                // requires an existing staged operation to finalize.
                if !data_store_path.exists() {
                    return Err(TridentError::new(InvalidInputError::HostNotProvisioned))
                        .message("Datastore file does not exist");
                }

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
