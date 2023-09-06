use anyhow::{Context, Error};
use datastore::DataStore;
use protobufs::*;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use tonic::transport::Server;
use tonic::{Request, Response, Status};
use trident_api::config::HostConfiguration;

mod datastore;
mod modules;
mod mount;

pub use modules::network::provisioning::start as start_provisioning_network;

mod protobufs {
    tonic::include_proto!("trident");
}

pub async fn serve(addr: IpAddr, port: u16) -> Result<(), tonic::transport::Error> {
    Server::builder()
        .add_service(imaging_server::ImagingServer::new(ImagingImpl))
        .serve(SocketAddr::new(addr, port))
        .await
}

#[derive(Default)]
pub struct ImagingImpl;

#[tonic::async_trait]
impl imaging_server::Imaging for ImagingImpl {
    async fn write_image(
        &self,
        request: Request<ImageRequest>,
    ) -> Result<Response<EmptyReply>, Status> {
        let _request = request.into_inner();
        // image::write_image(Path::new(&request.disk), &request.url, &request.sha256)
        //     .await
        //     .map_err(|e| Status::unknown(e.to_string()))?;

        Ok(Response::new(EmptyReply {}))
    }

    async fn chroot_exec(
        &self,
        request: Request<ChrootExecRequest>,
    ) -> Result<Response<EmptyReply>, Status> {
        let _request = request.into_inner();
        // image::chroot_exec(Path::new(&request.root_partition), &request.script)
        //     .await
        //     .map_err(|e| Status::unknown(e.to_string()))?;

        Ok(Response::new(EmptyReply {}))
    }

    async fn kexec(&self, request: Request<KexecRequest>) -> Result<Response<EmptyReply>, Status> {
        let _request = request.into_inner();
        // image::kexec(Path::new(&request.root_partition), &request.cmdline)
        //     .await
        //     .map_err(|e| Status::unknown(e.to_string()))?;
        unreachable!()
    }
}

pub fn run(host_config: &HostConfiguration, datastore: Option<PathBuf>) -> Result<(), Error> {
    match datastore {
        Some(path) => {
            let datastore = DataStore::open(&path).context("Failed to load datastore")?;
            modules::update_host_config(host_config, datastore)
                .context("Failed to update host config")
        }
        None => modules::provision(host_config).context("Failed to provision"),
    }
}
