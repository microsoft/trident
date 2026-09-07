use tonic::{async_trait, Request, Response, Status};

use trident_api::{
    config::{HostConfigurationSource, Operation, Operations},
    error::{InvalidInputError, TridentError, TridentResultExt},
};
use trident_proto::v1preview::{
    install_service_server::InstallService, FinalizeInstallRequest, InstallRequest,
    StageInstallRequest,
};

use crate::{
    server::{
        tridentserver::{RebootDecision, ServicingResponseStream},
        TridentServer,
    },
    validation, DataStore, Trident,
};

#[async_trait]
impl InstallService for TridentServer {
    type InstallStream = ServicingResponseStream;
    async fn install(
        &self,
        request: Request<InstallRequest>,
    ) -> Result<Response<Self::InstallStream>, Status> {
        let req = request.into_inner();
        let Some(staging) = req.stage else {
            return Err(self.reject_invalid_argument(
                "install",
                "stage",
                "Missing staging configuration",
            ));
        };

        let Some(host_config) = staging.config else {
            return Err(self.reject_invalid_argument(
                "install",
                "stage.config",
                "Missing host configuration in staging configuration",
            ));
        };

        let Some(finalize) = req.finalize else {
            return Err(self.reject_invalid_argument(
                "install",
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
            return Err(self.reject_invalid_config("install", e, message));
        }

        let data_store_path = self.agent_config.datastore_path().to_owned();
        let logstream = self.logstream.clone();
        let tracestream = self.tracestream.clone();

        self.servicing_request(
            "install",
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
                    .install(&mut datastore, Operations::all(), false, None)
                    .map(|(k, h, st)| (k, h, Some(st.into())))
            },
        )
    }

    type InstallStageStream = ServicingResponseStream;
    async fn install_stage(
        &self,
        request: Request<StageInstallRequest>,
    ) -> Result<Response<Self::InstallStageStream>, Status> {
        let req = request.into_inner();

        let Some(host_config) = req.config else {
            return Err(self.reject_invalid_argument(
                "install_stage",
                "config",
                "Missing host configuration in staging configuration",
            ));
        };

        // See the equivalent check in install() above for why this must
        // happen before servicing_request's datastore-creating pre-warm.
        if let Err(e) = validation::parse_host_config(&host_config.config, None::<&std::path::Path>)
        {
            let message = format!("Invalid host configuration: {e:?}");
            return Err(self.reject_invalid_config("install_stage", e, message));
        }

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

            trident
                .install(&mut datastore, Operation::Stage.into(), false, None)
                .map(|(k, h, st)| (k, h, Some(st.into())))
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
                    .install(&mut datastore, Operation::Finalize.into(), false, None)
                    .map(|(k, h, st)| (k, h, Some(st.into())))
            },
        )
    }
}
