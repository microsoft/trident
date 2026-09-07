use tonic::{async_trait, Request, Response, Status};

use trident_api::{
    config::{HostConfigurationSource, Operation, Operations},
    error::TridentResultExt,
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
            return Err(Status::invalid_argument(
                "Missing host configuration in staging configuration",
            ));
        };

        // See the equivalent check in install() above for why this must
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
