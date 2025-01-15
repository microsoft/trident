use std::net::SocketAddr;

use anyhow::{Context, Error};
use log::info;
use tokio::{
    runtime::Runtime,
    sync::mpsc::{self, Sender},
};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tonic::{transport::Server, Request, Response, Status};

use osutils::dependencies::Dependency;
use trident_api::{
    config::{GrpcConfiguration, Operations},
    error::{InternalError, ReportError, ServicingError, TridentError},
};

use crate::{datastore::DataStore, HostUpdateCommand, OrchestratorConnection};

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

pub(crate) fn send_host_status_state(
    sender: &mut Option<mpsc::UnboundedSender<Result<HostStatusState, tonic::Status>>>,
    state: &DataStore,
) -> Result<(), TridentError> {
    if let Some(ref mut sender) = sender {
        sender
            .send(Ok(HostStatusState {
                status: serde_yaml::to_string(state.host_status())
                    .structured(trident_api::error::InternalError::SerializeHostStatus)?,
            }))
            .structured(trident_api::error::InternalError::SendHostStatus)?;
    }
    Ok(())
}

fn open_firewall_for_grpc() -> Result<(), Error> {
    Dependency::Iptables
        .cmd()
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
