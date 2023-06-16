use protobufs::*;
use std::net::{IpAddr, SocketAddr};
use tonic::transport::Server;
use tonic::{Request, Response, Status};

pub mod config;
mod image;

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
        image::write_image(request.disk, request.url)
            .await
            .map_err(|e| Status::unknown(e.to_string()))?;

        Ok(Response::new(EmptyReply {}))
    }

    async fn chroot_exec(
        &self,
        request: Request<ChrootExecRequest>,
    ) -> Result<Response<EmptyReply>, Status> {
        let request = request.into_inner();
        image::chroot_exec(request.root_partition, request.script)
            .await
            .map_err(|e| Status::unknown(e.to_string()))?;

        Ok(Response::new(EmptyReply {}))
    }

    async fn kexec(&self, request: Request<KexecRequest>) -> Result<Response<EmptyReply>, Status> {
        let request = request.into_inner();
        image::kexec(request.root_partition, request.cmdline)
            .await
            .map_err(|e| Status::unknown(e.to_string()))?;
        unreachable!()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}
