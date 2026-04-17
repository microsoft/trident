use tonic::{async_trait, Request, Response, Status};

use trident_api::error::TridentResultExt;
use trident_proto::v1preview::{
    rebuild_raid_service_server::RebuildRaidService, RebuildRaidRequest,
};

use crate::{
    server::{
        tridentserver::{datastore, RebootDecision, ServicingResponseStream},
        TridentServer,
    },
    DataStore, ExitKind, Trident,
};

#[async_trait]
impl RebuildRaidService for TridentServer {
    type RebuildRaidStream = ServicingResponseStream;
    async fn rebuild_raid(
        &self,
        _request: Request<RebuildRaidRequest>,
    ) -> Result<Response<Self::RebuildRaidStream>, Status> {
        let data_store_path = self.agent_config.datastore_path().to_owned();
        let logstream = self.logstream.clone();
        let tracestream = self.tracestream.clone();

        self.servicing_request("rebuild_raid", RebootDecision::Error, move || {
            let mut trident = Trident::new(None, &data_store_path, logstream, tracestream)
                .message("Failed to initialize Trident")?;

            let mut datastore =
                DataStore::open_or_create(&data_store_path).message("Failed to open datastore")?;

            let image_hash = datastore::stored_image_hash(&datastore);

            trident
                .rebuild_raid(&mut datastore)
                .message("Failed to rebuild RAID arrays")?;

            Ok((ExitKind::Done, image_hash, None))
        })
    }
}
