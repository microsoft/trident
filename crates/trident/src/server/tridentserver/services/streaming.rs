use tonic::{async_trait, Request, Response, Status};
use url::Url;

use trident_api::{constants::IMAGE_CHECKSUM_IGNORED, error::TridentResultExt};
use trident_proto::v1::{streaming_service_server::StreamingService, StreamDiskRequest};

use crate::{
    server::{tridentserver::ServicingResponseStream, TridentServer},
    DataStore, Trident,
};

#[async_trait]
impl StreamingService for TridentServer {
    type StreamDiskStream = ServicingResponseStream;
    async fn stream_disk(
        &self,
        request: Request<StreamDiskRequest>,
    ) -> Result<Response<Self::StreamDiskStream>, Status> {
        let req = request.into_inner();

        // Parse the image URL from the request, returning an error if it is invalid.
        let url = Url::parse(&req.image_url).map_err(|e| {
            Status::invalid_argument(format!("Invalid image URL '{}': {}", req.image_url, e))
        })?;

        // If the image hash is not provided, we use the constant for ignored checksum.
        let image_hash = req
            .image_hash
            .clone()
            .unwrap_or_else(|| IMAGE_CHECKSUM_IGNORED.into());

        let data_store_path = self.agent_config.datastore_path().to_owned();
        let logstream = self.logstream.clone();
        let tracestream = self.tracestream.clone();

        self.servicing_request(
            "stream_disk",
            super::reboot_allowed(&req.reboot),
            move || {
                let mut trident = Trident::new(None, &data_store_path, logstream, tracestream)
                    .message("Failed to initialize Trident")?;

                let mut datastore = DataStore::open_or_create(&data_store_path)
                    .message("Failed to open datastore")?;

                trident.stream_image(&mut datastore, &url, &image_hash)
            },
        )
    }
}
