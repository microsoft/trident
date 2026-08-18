//! In-process mock tridentd used only by unit tests.
//!
//! Implements the real generated `UpdateService`/`CommitService` server
//! traits (gated behind trident-proto's `server` feature, enabled only in
//! `[dev-dependencies]` - see trident-acl-agent/Cargo.toml) so tests can
//! exercise the *real* `TridentClient` request/response/error-mapping code
//! against canned stage/finalize/commit outcomes, without a real tridentd
//! process or unix socket.
//!
//! Tests wire a `TridentClient` to this mock server over an in-memory
//! `tokio::io::duplex` transport via `Endpoint::connect_with_connector` +
//! `TridentClient::from_channel` - see `connect_mock_client` below.

use std::sync::{Arc, Mutex};

use hyper_util::rt::TokioIo;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{transport::Endpoint, Request, Response, Status};
use trident_proto::v1::{
    commit_service_server::{CommitService, CommitServiceServer},
    rollback_service_server::{RollbackService, RollbackServiceServer},
    servicing_response::Response as ResponseBody,
    update_service_server::{UpdateService, UpdateServiceServer},
    CommitRequest, Completed, FinalizeUpdateRequest, RebootStatus, RollbackFinalizeRequest,
    RollbackRequest, RollbackStageRequest, ServicingKind, ServicingResponse, StageUpdateRequest,
    StatusCode as ProtoStatusCode, TridentError, UpdateRequest,
};

use trident_agent_core::trident::TridentClient;

/// Canned outcome a `MockTridentd` should return for a given RPC call.
#[derive(Clone, Debug)]
pub enum Outcome {
    /// Respond with a successful `Completed` message. `servicing_kind`
    /// mirrors what a real tridentd populates on every servicing RPC
    /// (`ServicingKind::NoneRequired` for a no-op, the real kind
    /// otherwise) - tests that care about the no-op-detection path (see
    /// orchestrator.rs's `handle_rollback`) set this explicitly; other
    /// tests that don't inspect it can pass `None`.
    Success {
        reboot_status: RebootStatus,
        servicing_kind: Option<ServicingKind>,
    },
    /// Respond with a failed `Completed` message carrying the given error
    /// subkind (e.g. "ab-update-reboot-check").
    Failure {
        subkind: &'static str,
        message: &'static str,
    },
}

impl Outcome {
    fn into_servicing_response(self) -> ServicingResponse {
        let completed = match self {
            Outcome::Success {
                reboot_status,
                servicing_kind,
            } => Completed {
                status: ProtoStatusCode::Success as i32,
                error: None,
                reboot_status: reboot_status as i32,
                image_hash: None,
                servicing_kind: servicing_kind.map(|k| k as i32),
            },
            Outcome::Failure { subkind, message } => Completed {
                status: ProtoStatusCode::Failure as i32,
                error: Some(TridentError {
                    kind: 0,
                    subkind: subkind.to_string(),
                    message: message.to_string(),
                    error_message: message.to_string(),
                    location: None,
                }),
                reboot_status: RebootStatus::Unspecified as i32,
                image_hash: None,
                servicing_kind: None,
            },
        };
        ServicingResponse {
            timestamp: None,
            response: Some(ResponseBody::Completed(completed)),
        }
    }
}

/// Configurable canned responses for the three RPCs `TridentClient` calls.
/// Each field defaults to `None`; a test sets only the outcome(s) it cares
/// about, and the mock server panics if a call arrives with no outcome
/// configured (surfacing test-setup bugs immediately rather than silently
/// hanging or defaulting).
#[derive(Default)]
pub struct MockTridentdConfig {
    pub stage: Option<Outcome>,
    pub finalize: Option<Outcome>,
    pub commit: Option<Outcome>,
    pub rollback_stage: Option<Outcome>,
    pub rollback_finalize: Option<Outcome>,
}

#[derive(Clone)]
struct MockTridentd {
    config: Arc<Mutex<MockTridentdConfig>>,
}

async fn respond_with(
    outcome: Outcome,
) -> Result<Response<ReceiverStream<Result<ServicingResponse, Status>>>, Status> {
    let (tx, rx) = tokio::sync::mpsc::channel(4);
    tx.send(Ok(outcome.into_servicing_response()))
        .await
        .expect("mock tridentd channel send should not fail");
    Ok(Response::new(ReceiverStream::new(rx)))
}

#[tonic::async_trait]
impl UpdateService for MockTridentd {
    type UpdateStream = ReceiverStream<Result<ServicingResponse, Status>>;
    type UpdateStageStream = ReceiverStream<Result<ServicingResponse, Status>>;
    type UpdateFinalizeStream = ReceiverStream<Result<ServicingResponse, Status>>;

    async fn update(
        &self,
        _request: Request<UpdateRequest>,
    ) -> Result<Response<Self::UpdateStream>, Status> {
        Err(Status::unimplemented(
            "update() is not used by trident-acl-agent",
        ))
    }

    async fn update_stage(
        &self,
        _request: Request<StageUpdateRequest>,
    ) -> Result<Response<Self::UpdateStageStream>, Status> {
        let outcome =
            self.config.lock().unwrap().stage.clone().expect(
                "test must configure MockTridentdConfig::stage before calling update_stage",
            );
        respond_with(outcome).await
    }

    async fn update_finalize(
        &self,
        _request: Request<FinalizeUpdateRequest>,
    ) -> Result<Response<Self::UpdateFinalizeStream>, Status> {
        let outcome = self.config.lock().unwrap().finalize.clone().expect(
            "test must configure MockTridentdConfig::finalize before calling update_finalize",
        );
        respond_with(outcome).await
    }
}

#[tonic::async_trait]
impl CommitService for MockTridentd {
    type CommitStream = ReceiverStream<Result<ServicingResponse, Status>>;

    async fn commit(
        &self,
        _request: Request<CommitRequest>,
    ) -> Result<Response<Self::CommitStream>, Status> {
        let outcome = self
            .config
            .lock()
            .unwrap()
            .commit
            .clone()
            .expect("test must configure MockTridentdConfig::commit before calling commit");
        respond_with(outcome).await
    }
}

#[tonic::async_trait]
impl RollbackService for MockTridentd {
    type RollbackStream = ReceiverStream<Result<ServicingResponse, Status>>;
    type RollbackStageStream = ReceiverStream<Result<ServicingResponse, Status>>;
    type RollbackFinalizeStream = ReceiverStream<Result<ServicingResponse, Status>>;

    // check_rollback is no longer part of the stable v1 RollbackService
    // trait (demoted back to trident.v1preview - trident-acl-agent detects
    // a no-op rollback via RollbackStage's servicing_kind now instead, see
    // orchestrator.rs's handle_rollback), so this mock no longer needs to
    // implement it.

    async fn rollback(
        &self,
        _request: Request<RollbackRequest>,
    ) -> Result<Response<Self::RollbackStream>, Status> {
        Err(Status::unimplemented(
            "rollback() is not used by trident-acl-agent",
        ))
    }

    async fn rollback_stage(
        &self,
        _request: Request<RollbackStageRequest>,
    ) -> Result<Response<Self::RollbackStageStream>, Status> {
        let outcome = self.config.lock().unwrap().rollback_stage.clone().expect(
            "test must configure MockTridentdConfig::rollback_stage before calling rollback_stage",
        );
        respond_with(outcome).await
    }

    async fn rollback_finalize(
        &self,
        _request: Request<RollbackFinalizeRequest>,
    ) -> Result<Response<Self::RollbackFinalizeStream>, Status> {
        let outcome = self
            .config
            .lock()
            .unwrap()
            .rollback_finalize
            .clone()
            .expect(
            "test must configure MockTridentdConfig::rollback_finalize before calling rollback_finalize",
        );
        respond_with(outcome).await
    }
}

/// Starts an in-process mock tridentd wired to `client` over an in-memory
/// duplex transport (no real socket/subprocess), and returns a
/// `TridentClient` connected to it. `config` is shared (`Arc<Mutex<..>>`)
/// so the caller can reconfigure outcomes between calls if a test needs to
/// simulate stage-then-finalize-then-commit in one session.
pub async fn connect_mock_client(config: Arc<Mutex<MockTridentdConfig>>) -> TridentClient {
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);

    let mock = MockTridentd { config };
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(UpdateServiceServer::new(mock.clone()))
            .add_service(CommitServiceServer::new(mock.clone()))
            .add_service(RollbackServiceServer::new(mock))
            .serve_with_incoming(tokio_stream::once(Ok::<_, std::io::Error>(server_io)))
            .await
            .expect("mock tridentd server should not fail");
    });

    let mut client_io = Some(client_io);
    let channel = Endpoint::try_from("http://[::]:50051")
        .expect("static endpoint URI should always parse")
        .connect_with_connector(tower::service_fn(move |_: tonic::transport::Uri| {
            let client_io = client_io.take();
            async move {
                client_io.map(TokioIo::new).ok_or_else(|| {
                    std::io::Error::other("mock client connector called more than once")
                })
            }
        }))
        .await
        .expect("in-memory duplex connection should succeed");

    TridentClient::from_channel(channel)
}
