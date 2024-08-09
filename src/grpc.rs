use std::net::SocketAddr;
use std::process::Command;

use anyhow::Context;
use anyhow::Error;
use log::info;
use tokio::runtime::Runtime;
use tokio::sync::mpsc::{self, Sender};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tonic::transport::Server;
use tonic::{Request, Response, Status};

use osutils::exe::RunAndCheck;
use trident_api::config::{GrpcConfiguration, Operations};
use trident_api::error::{InternalError, ReportError};
use trident_api::error::{ServicingError, TridentError};

use crate::{HostUpdateCommand, OrchestratorConnection};

pub mod protobufs {
    tonic::include_proto!("trident");
}
pub use protobufs::*;

/// Implementation of the gRPC service.
///
/// This struct contains a tokio Sender which it uses to enqueue commands to the main Trident
/// thread. It also implements the gRPC service trait, which allows it to be used as a gRPC server.
pub struct HostManagementImpl(Sender<HostUpdateCommand>);

#[tonic::async_trait]
impl host_management_server::HostManagement for HostManagementImpl {
    type UpdateHostStream = UnboundedReceiverStream<Result<HostStatusState, Status>>;

    async fn update_host(
        &self,
        request: Request<HostUpdateRequest>,
    ) -> Result<Response<Self::UpdateHostStream>, Status> {
        info!("Received update_host request");
        let request = request.into_inner();

        let host_config = serde_yaml::from_str(&request.host_configuration)
            .context("Failed to parse host config")
            .map_err(|e| Status::invalid_argument(format!("{e:?}")))?;

        let (tx, rx) = mpsc::unbounded_channel();
        self.0
            .send(HostUpdateCommand {
                allowed_operations: Operations::default(), // TODO
                host_config,
                sender: Some(tx),
            })
            .await
            .context("Failed to enqueue 'HostUpdate' command to the main Trident thread")
            .map_err(|e| Status::from_error(e.into()))?;

        Ok(Response::new(UnboundedReceiverStream::new(rx)))
    }
}

/// Start the gRPC server.
pub(crate) fn start(
    grpc: &GrpcConfiguration,
    orchestrator: Option<&OrchestratorConnection>,
    sender: Sender<HostUpdateCommand>,
) -> Result<Runtime, TridentError> {
    // TODO: make firewall this configurable
    info!("Opening firewall");
    let _ = open_firewall_for_grpc().structured(ServicingError::OpenFirewall);

    let addr = "0.0.0.0".parse().unwrap();
    let port = grpc.listen_port.unwrap_or(50051);

    info!("Preparing to listen for gRPC requests");

    let rt = tokio::runtime::Runtime::new().structured(InternalError::StartTokioRuntime)?;
    rt.spawn(async move {
        Server::builder()
            .add_service(host_management_server::HostManagementServer::new(
                HostManagementImpl(sender),
            ))
            .serve(SocketAddr::new(addr, port))
            .await
            .context("Failed while serving gRPC requests")
    });

    // Notify orchestrator that we are ready to receive commands.
    if let Some(orchestrator) = orchestrator {
        orchestrator.report_success(None)
    }

    Ok(rt)
}

fn open_firewall_for_grpc() -> Result<(), Error> {
    Command::new("iptables")
        .arg("-A")
        .arg("INPUT")
        .arg("-p")
        .arg("tcp")
        .arg("--dport")
        .arg("50051") // TODO
        .arg("-j")
        .arg("ACCEPT")
        .run_and_check()
        .context("Failed to open firewall for gRPC")
}
