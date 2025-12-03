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
    config::{GrpcConfiguration, HostConfiguration, Operations},
    error::{InternalError, ReportError, ServicingError, TridentError, TridentResultExt},
};

use crate::{datastore::DataStore, OrchestratorConnection};

pub mod protobufs {
    tonic::include_proto!("trident");
}
pub use protobufs::*;

pub type GrpcSender = mpsc::UnboundedSender<Result<HostStatusState, tonic::Status>>;

/// Implementation of the gRPC service.
///
/// This struct contains a tokio Sender which it uses to enqueue commands to the main Trident
/// thread. It also implements the gRPC service trait, which allows it to be used as a gRPC server.
pub struct HostManagementImpl(Sender<(HostConfiguration, Operations, GrpcSender)>);

#[tonic::async_trait]
impl host_management_server::HostManagement for HostManagementImpl {
    type UpdateHostStream = UnboundedReceiverStream<Result<HostStatusState, Status>>;

    async fn cosi_to_host_configuration(
        &self,
        request: Request<CosiToHostConfigurationRequest>,
    ) -> Result<Response<CosiToHostConfigurationResponse>, Status> {
        info!("Received cosi_to_host_configuration request");
        let request = request.into_inner();

        let host_config = crate::stream::config_from_image_url(
            request.cosi_url.parse().map_err(|e| {
                Status::invalid_argument(format!("Failed to parse COSI URL: {e:?}"))
            })?,
            &request.cosi_hash,
        )
        .unstructured("Failed to convert COSI to Host Configuration")
        .map_err(|e| Status::internal(format!("{e:?}")))?;

        let response = CosiToHostConfigurationResponse {
            host_configuration: serde_yaml::to_string(&host_config)
                .context("Failed to serialize Host Configuration")
                .map_err(|e| Status::internal(format!("{e:?}")))?,
        };

        Ok(Response::new(response))
    }

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
            .send((host_config, Operations::all(), tx))
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
    sender: Sender<(HostConfiguration, Operations, GrpcSender)>,
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
    if let Some(sender) = sender {
        sender
            .send(Ok(HostStatusState {
                status: serde_yaml::to_string(state.host_status())
                    .structured(InternalError::SerializeHostStatus)?,
            }))
            .structured(InternalError::SendHostStatus)?;
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
