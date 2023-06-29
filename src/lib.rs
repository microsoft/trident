use config::HostConfig;
use protobufs::*;
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use tonic::transport::Server;
use tonic::{Request, Response, Status};

pub mod config;
mod image;
mod network;

pub use network::provisioning::start as start_provisioning_network;

mod protobufs {
    tonic::include_proto!("trident");
}

pub async fn serve(addr: IpAddr, port: u16) -> Result<(), tonic::transport::Error> {
    Server::builder()
        .add_service(imaging_server::ImagingServer::new(ImagingImpl::default()))
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
        let request = request.into_inner();
        image::write_image(Path::new(&request.disk), &request.url, &request.sha256)
            .await
            .map_err(|e| Status::unknown(e.to_string()))?;

        Ok(Response::new(EmptyReply {}))
    }

    async fn chroot_exec(
        &self,
        request: Request<ChrootExecRequest>,
    ) -> Result<Response<EmptyReply>, Status> {
        let request = request.into_inner();
        image::chroot_exec(Path::new(&request.root_partition), &request.script)
            .await
            .map_err(|e| Status::unknown(e.to_string()))?;

        Ok(Response::new(EmptyReply {}))
    }

    async fn kexec(&self, request: Request<KexecRequest>) -> Result<Response<EmptyReply>, Status> {
        let request = request.into_inner();
        image::kexec(Path::new(&request.root_partition), &request.cmdline)
            .await
            .map_err(|e| Status::unknown(e.to_string()))?;
        unreachable!()
    }
}

pub async fn auto_provision(host_config: &HostConfig) -> Result<(), Box<dyn std::error::Error>> {
    image::write_image(
        &host_config.disk.device,
        &host_config.disk.image_url,
        &host_config.disk.image_sha256,
    )
    .await?;

    image::chroot_exec(
        &host_config.disk.partition,
        "useradd -p $(openssl passwd -1 tink) -s /bin/bash -d /home/tink/ -m -G sudo tink",
    )
    .await?;

    image::kexec(&host_config.disk.partition, "console=tty1 console=ttyS0").await?;

    unreachable!()
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}
