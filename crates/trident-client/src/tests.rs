use super::*;

use std::{
    collections::VecDeque,
    io::Error as IoError,
    sync::{Arc, Mutex},
    time::Duration,
};

use hyper_util::rt::TokioIo;
use prost_types::Timestamp;
use tokio::{io, net::UnixListener, sync::mpsc, time};
use tokio_stream::wrappers::{ReceiverStream, UnixListenerStream};
use tonic::{
    transport::{Endpoint, Server, Uri},
    Request, Response, Status,
};
use tower::service_fn;

use trident_proto::v1::{
    commit_service_server::{CommitService, CommitServiceServer},
    rollback_service_server::{RollbackService, RollbackServiceServer},
    servicing_response::Response as ResponseBody,
    streaming_service_server::{StreamingService, StreamingServiceServer},
    update_service_server::{UpdateService, UpdateServiceServer},
    version_service_server::{VersionService, VersionServiceServer},
    CommitRequest, Completed, FileLocation, FinalizeUpdateRequest, Log, LogLevel as ProtoLogLevel,
    ManualRollbackKind, RebootHandling, RebootStatus, RollbackFinalizeRequest, RollbackRequest,
    RollbackStageRequest, ServicingKind as ProtoServicingKind, ServicingResponse,
    StageUpdateRequest, StatusCode, StreamDiskRequest, TridentError, TridentErrorKind,
    UpdateRequest, VersionRequest, VersionResponse,
};

#[cfg(feature = "preview")]
use trident_proto::v1preview::{
    install_service_server::{InstallService, InstallServiceServer},
    rebuild_raid_service_server::{RebuildRaidService, RebuildRaidServiceServer},
    rollback_service_server::{
        RollbackService as PreviewRollbackService,
        RollbackServiceServer as PreviewRollbackServiceServer,
    },
    status_service_server::{StatusService, StatusServiceServer},
    validation_service_server::{ValidationService, ValidationServiceServer},
    AbVolumeState, CheckRollbackKind, CheckRollbackRequest, CheckRollbackResponse,
    FinalizeInstallRequest, GetActiveVolumeRequest, GetActiveVolumeResponse, GetConfigRequest,
    GetConfigResponse, GetLastErrorRequest, GetLastErrorResponse, GetRequiredServicingTypeRequest,
    GetRequiredServicingTypeResponse, GetRollbackChainRequest, GetRollbackChainResponse,
    GetRollbackTargetRequest, GetRollbackTargetResponse, GetServicingStateRequest,
    GetServicingStateResponse, InstallRequest, RebuildRaidRequest,
    ServicingState as ProtoServicingState, StageInstallRequest, ValidateHostConfigurationRequest,
    ValidateHostConfigurationResponse,
};

const DUPLEX_BUFFER_BYTES: usize = 64 * 1024;
const RESPONSE_CHANNEL_CAPACITY: usize = 8;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
const SHORT_TIMEOUT: Duration = Duration::from_millis(20);
const LONG_DELAY: Duration = Duration::from_millis(100);
const SHA384: &str =
    "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

type ResponseStream = ReceiverStream<Result<ServicingResponse, Status>>;

enum StreamStep {
    Item(Result<ServicingResponse, Status>),
    Delay(Duration),
}

enum Action {
    Stream(Vec<StreamStep>),
    Reject(Status),
    DelayStart(Duration, Vec<StreamStep>),
}

#[derive(Default)]
struct Captured {
    version_requests: usize,
    update_yaml: Vec<String>,
    update_reboots: Vec<i32>,
    update_stage_yaml: Vec<String>,
    update_finalize_reboots: Vec<i32>,
    stream_disk: Vec<(String, Option<String>, i32)>,
    commit_reboots: Vec<i32>,
    rollbacks: Vec<(i32, i32)>,
    rollback_stages: Vec<i32>,
    rollback_finalizes: Vec<i32>,
    #[cfg(feature = "preview")]
    install_yaml: Vec<String>,
    #[cfg(feature = "preview")]
    install_reboots: Vec<i32>,
    #[cfg(feature = "preview")]
    install_stage_yaml: Vec<String>,
    #[cfg(feature = "preview")]
    install_finalize_reboots: Vec<i32>,
    #[cfg(feature = "preview")]
    rebuild_raid_requests: usize,
    #[cfg(feature = "preview")]
    servicing_state_requests: usize,
    #[cfg(feature = "preview")]
    active_volume_requests: usize,
    #[cfg(feature = "preview")]
    validation_yaml: Vec<String>,
    #[cfg(feature = "preview")]
    check_rollback_kinds: Vec<i32>,
}

struct MockState {
    actions: Mutex<VecDeque<Action>>,
    captured: Mutex<Captured>,
    version_delay: Mutex<Option<Duration>>,
    #[cfg(feature = "preview")]
    servicing_state: Mutex<i32>,
    #[cfg(feature = "preview")]
    active_volume: Mutex<i32>,
    #[cfg(feature = "preview")]
    validation_response: Mutex<ValidateHostConfigurationResponse>,
    #[cfg(feature = "preview")]
    check_rollback_kind: Mutex<i32>,
}

impl Default for MockState {
    fn default() -> Self {
        Self {
            actions: Mutex::new(VecDeque::new()),
            captured: Mutex::new(Captured::default()),
            version_delay: Mutex::new(None),
            #[cfg(feature = "preview")]
            servicing_state: Mutex::new(ProtoServicingState::Provisioned.into()),
            #[cfg(feature = "preview")]
            active_volume: Mutex::new(AbVolumeState::VolumeA.into()),
            #[cfg(feature = "preview")]
            validation_response: Mutex::new(ValidateHostConfigurationResponse {
                ok: true,
                error: None,
            }),
            #[cfg(feature = "preview")]
            check_rollback_kind: Mutex::new(CheckRollbackKind::AbRollbackExpected.into()),
        }
    }
}

#[derive(Clone)]
struct MockServer {
    state: Arc<MockState>,
}

impl MockServer {
    async fn respond(&self) -> Result<Response<ResponseStream>, Status> {
        let action = self
            .state
            .actions
            .lock()
            .unwrap()
            .pop_front()
            .expect("test must configure a mock action");
        let steps = match action {
            Action::Reject(status) => return Err(status),
            Action::Stream(steps) => steps,
            Action::DelayStart(delay, steps) => {
                time::sleep(delay).await;
                steps
            }
        };
        let (sender, receiver) = mpsc::channel(RESPONSE_CHANNEL_CAPACITY);
        tokio::spawn(async move {
            for step in steps {
                match step {
                    StreamStep::Item(item) => {
                        if sender.send(item).await.is_err() {
                            return;
                        }
                    }
                    StreamStep::Delay(delay) => time::sleep(delay).await,
                }
            }
        });
        Ok(Response::new(ReceiverStream::new(receiver)))
    }
}

#[tonic::async_trait]
impl VersionService for MockServer {
    async fn version(
        &self,
        _request: Request<VersionRequest>,
    ) -> Result<Response<VersionResponse>, Status> {
        self.state.captured.lock().unwrap().version_requests += 1;
        let delay = self.state.version_delay.lock().unwrap().take();
        if let Some(delay) = delay {
            time::sleep(delay).await;
        }
        Ok(Response::new(VersionResponse {
            version: "test-version".to_string(),
        }))
    }
}

#[tonic::async_trait]
impl UpdateService for MockServer {
    type UpdateStream = ResponseStream;
    type UpdateStageStream = ResponseStream;
    type UpdateFinalizeStream = ResponseStream;

    async fn update(
        &self,
        request: Request<UpdateRequest>,
    ) -> Result<Response<Self::UpdateStream>, Status> {
        let request = request.into_inner();
        let yaml = request
            .stage
            .and_then(|stage| stage.config)
            .expect("client should send update config")
            .config;
        let reboot = request
            .finalize
            .and_then(|finalize| finalize.reboot)
            .expect("client should send update reboot policy")
            .handling;
        {
            let mut captured = self.state.captured.lock().unwrap();
            captured.update_yaml.push(yaml);
            captured.update_reboots.push(reboot);
        }
        self.respond().await
    }

    async fn update_stage(
        &self,
        request: Request<StageUpdateRequest>,
    ) -> Result<Response<Self::UpdateStageStream>, Status> {
        let yaml = request
            .into_inner()
            .config
            .expect("client should send update-stage config")
            .config;
        self.state
            .captured
            .lock()
            .unwrap()
            .update_stage_yaml
            .push(yaml);
        self.respond().await
    }

    async fn update_finalize(
        &self,
        request: Request<FinalizeUpdateRequest>,
    ) -> Result<Response<Self::UpdateFinalizeStream>, Status> {
        let reboot = request
            .into_inner()
            .reboot
            .expect("client should send update-finalize reboot policy")
            .handling;
        self.state
            .captured
            .lock()
            .unwrap()
            .update_finalize_reboots
            .push(reboot);
        self.respond().await
    }
}

#[tonic::async_trait]
impl CommitService for MockServer {
    type CommitStream = ResponseStream;

    async fn commit(
        &self,
        request: Request<CommitRequest>,
    ) -> Result<Response<Self::CommitStream>, Status> {
        let reboot = request
            .into_inner()
            .reboot
            .expect("client should send commit reboot policy")
            .handling;
        self.state
            .captured
            .lock()
            .unwrap()
            .commit_reboots
            .push(reboot);
        self.respond().await
    }
}

#[tonic::async_trait]
impl StreamingService for MockServer {
    type StreamDiskStream = ResponseStream;

    async fn stream_disk(
        &self,
        request: Request<StreamDiskRequest>,
    ) -> Result<Response<Self::StreamDiskStream>, Status> {
        let request = request.into_inner();
        let reboot = request
            .reboot
            .expect("client should send stream-disk reboot policy")
            .handling;
        self.state.captured.lock().unwrap().stream_disk.push((
            request.image_url,
            request.image_hash,
            reboot,
        ));
        self.respond().await
    }
}

#[tonic::async_trait]
impl RollbackService for MockServer {
    type RollbackStream = ResponseStream;
    type RollbackStageStream = ResponseStream;
    type RollbackFinalizeStream = ResponseStream;

    async fn rollback(
        &self,
        request: Request<RollbackRequest>,
    ) -> Result<Response<Self::RollbackStream>, Status> {
        let request = request.into_inner();
        let kind = request
            .stage
            .expect("client should send rollback stage")
            .kind;
        let reboot = request
            .finalize
            .and_then(|finalize| finalize.reboot)
            .expect("client should send rollback reboot policy")
            .handling;
        self.state
            .captured
            .lock()
            .unwrap()
            .rollbacks
            .push((kind, reboot));
        self.respond().await
    }

    async fn rollback_stage(
        &self,
        request: Request<RollbackStageRequest>,
    ) -> Result<Response<Self::RollbackStageStream>, Status> {
        self.state
            .captured
            .lock()
            .unwrap()
            .rollback_stages
            .push(request.into_inner().kind);
        self.respond().await
    }

    async fn rollback_finalize(
        &self,
        request: Request<RollbackFinalizeRequest>,
    ) -> Result<Response<Self::RollbackFinalizeStream>, Status> {
        let reboot = request
            .into_inner()
            .reboot
            .expect("client should send rollback-finalize reboot policy")
            .handling;
        self.state
            .captured
            .lock()
            .unwrap()
            .rollback_finalizes
            .push(reboot);
        self.respond().await
    }
}

#[cfg(feature = "preview")]
#[tonic::async_trait]
impl InstallService for MockServer {
    type InstallStream = ResponseStream;
    type InstallStageStream = ResponseStream;
    type InstallFinalizeStream = ResponseStream;

    async fn install(
        &self,
        request: Request<InstallRequest>,
    ) -> Result<Response<Self::InstallStream>, Status> {
        let request = request.into_inner();
        let yaml = request
            .stage
            .and_then(|stage| stage.config)
            .expect("client should send install config")
            .config;
        let reboot = request
            .finalize
            .and_then(|finalize| finalize.reboot)
            .expect("client should send install reboot policy")
            .handling;
        {
            let mut captured = self.state.captured.lock().unwrap();
            captured.install_yaml.push(yaml);
            captured.install_reboots.push(reboot);
        }
        self.respond().await
    }

    async fn install_stage(
        &self,
        request: Request<StageInstallRequest>,
    ) -> Result<Response<Self::InstallStageStream>, Status> {
        let yaml = request
            .into_inner()
            .config
            .expect("client should send install-stage config")
            .config;
        self.state
            .captured
            .lock()
            .unwrap()
            .install_stage_yaml
            .push(yaml);
        self.respond().await
    }

    async fn install_finalize(
        &self,
        request: Request<FinalizeInstallRequest>,
    ) -> Result<Response<Self::InstallFinalizeStream>, Status> {
        let reboot = request
            .into_inner()
            .reboot
            .expect("client should send install-finalize reboot policy")
            .handling;
        self.state
            .captured
            .lock()
            .unwrap()
            .install_finalize_reboots
            .push(reboot);
        self.respond().await
    }
}

#[cfg(feature = "preview")]
#[tonic::async_trait]
impl RebuildRaidService for MockServer {
    type RebuildRaidStream = ResponseStream;

    async fn rebuild_raid(
        &self,
        _request: Request<RebuildRaidRequest>,
    ) -> Result<Response<Self::RebuildRaidStream>, Status> {
        self.state.captured.lock().unwrap().rebuild_raid_requests += 1;
        self.respond().await
    }
}

#[cfg(feature = "preview")]
#[tonic::async_trait]
impl StatusService for MockServer {
    async fn get_provisioned_config(
        &self,
        _request: Request<GetConfigRequest>,
    ) -> Result<Response<GetConfigResponse>, Status> {
        Err(Status::unimplemented("not exposed by client"))
    }

    async fn get_servicing_config(
        &self,
        _request: Request<GetConfigRequest>,
    ) -> Result<Response<GetConfigResponse>, Status> {
        Err(Status::unimplemented("not exposed by client"))
    }

    async fn get_last_error(
        &self,
        _request: Request<GetLastErrorRequest>,
    ) -> Result<Response<GetLastErrorResponse>, Status> {
        Err(Status::unimplemented("not exposed by client"))
    }

    async fn get_servicing_state(
        &self,
        _request: Request<GetServicingStateRequest>,
    ) -> Result<Response<GetServicingStateResponse>, Status> {
        self.state.captured.lock().unwrap().servicing_state_requests += 1;
        Ok(Response::new(GetServicingStateResponse {
            state: *self.state.servicing_state.lock().unwrap(),
        }))
    }

    async fn get_active_volume(
        &self,
        _request: Request<GetActiveVolumeRequest>,
    ) -> Result<Response<GetActiveVolumeResponse>, Status> {
        self.state.captured.lock().unwrap().active_volume_requests += 1;
        Ok(Response::new(GetActiveVolumeResponse {
            active_volume: *self.state.active_volume.lock().unwrap(),
        }))
    }
}

#[cfg(feature = "preview")]
#[tonic::async_trait]
impl ValidationService for MockServer {
    async fn validate_host_configuration(
        &self,
        request: Request<ValidateHostConfigurationRequest>,
    ) -> Result<Response<ValidateHostConfigurationResponse>, Status> {
        let yaml = request
            .into_inner()
            .config
            .expect("client should send validation config")
            .config;
        self.state
            .captured
            .lock()
            .unwrap()
            .validation_yaml
            .push(yaml);
        Ok(Response::new(
            self.state.validation_response.lock().unwrap().clone(),
        ))
    }

    async fn get_required_servicing_type(
        &self,
        _request: Request<GetRequiredServicingTypeRequest>,
    ) -> Result<Response<GetRequiredServicingTypeResponse>, Status> {
        Err(Status::unimplemented("not exposed by client"))
    }
}

#[cfg(feature = "preview")]
#[tonic::async_trait]
impl PreviewRollbackService for MockServer {
    async fn check_rollback(
        &self,
        request: Request<CheckRollbackRequest>,
    ) -> Result<Response<CheckRollbackResponse>, Status> {
        self.state
            .captured
            .lock()
            .unwrap()
            .check_rollback_kinds
            .push(request.into_inner().kind);
        Ok(Response::new(CheckRollbackResponse {
            kind: *self.state.check_rollback_kind.lock().unwrap(),
        }))
    }

    async fn get_rollback_chain(
        &self,
        _request: Request<GetRollbackChainRequest>,
    ) -> Result<Response<GetRollbackChainResponse>, Status> {
        Err(Status::unimplemented("not exposed by client"))
    }

    async fn get_rollback_target(
        &self,
        _request: Request<GetRollbackTargetRequest>,
    ) -> Result<Response<GetRollbackTargetResponse>, Status> {
        Err(Status::unimplemented("not exposed by client"))
    }
}

async fn connect_mock(state: Arc<MockState>, request_timeout: Duration) -> Client {
    connect_mock_with_timeouts(state, request_timeout, request_timeout).await
}

async fn connect_mock_with_timeouts(
    state: Arc<MockState>,
    request_acknowledgement_timeout: Duration,
    unary_timeout: Duration,
) -> Client {
    let (client_io, server_io) = io::duplex(DUPLEX_BUFFER_BYTES);
    let mock = MockServer {
        state: state.clone(),
    };
    tokio::spawn(async move {
        let server = Server::builder()
            .add_service(VersionServiceServer::new(mock.clone()))
            .add_service(UpdateServiceServer::new(mock.clone()))
            .add_service(StreamingServiceServer::new(mock.clone()))
            .add_service(CommitServiceServer::new(mock.clone()))
            .add_service(RollbackServiceServer::new(mock.clone()));
        #[cfg(feature = "preview")]
        let server = server
            .add_service(InstallServiceServer::new(mock.clone()))
            .add_service(RebuildRaidServiceServer::new(mock.clone()))
            .add_service(StatusServiceServer::new(mock.clone()))
            .add_service(ValidationServiceServer::new(mock.clone()))
            .add_service(PreviewRollbackServiceServer::new(mock));
        server
            .serve_with_incoming(tokio_stream::once(Ok::<_, IoError>(server_io)))
            .await
            .expect("mock server should run");
    });

    let mut client_io = Some(client_io);
    let channel = Endpoint::from_static("http://[::]:50051")
        .connect_with_connector(service_fn(move |_: Uri| {
            let client_io = client_io.take();
            async move {
                client_io
                    .map(TokioIo::new)
                    .ok_or_else(|| IoError::other("mock connector reused"))
            }
        }))
        .await
        .expect("mock channel should connect");
    Client::from_channel(channel, request_acknowledgement_timeout, unary_timeout)
}

fn set_action(state: &Arc<MockState>, action: Action) {
    state.actions.lock().unwrap().push_back(action);
}

fn success() -> ServicingResponse {
    completed(Completed {
        status: StatusCode::Success.into(),
        error: None,
        reboot_status: RebootStatus::RebootNotRequired.into(),
        image_hash: None,
        servicing_kind: None,
    })
}

fn completed(completed: Completed) -> ServicingResponse {
    ServicingResponse {
        timestamp: None,
        response: Some(ResponseBody::Completed(completed)),
    }
}

fn stream(messages: Vec<ServicingResponse>) -> Action {
    Action::Stream(
        messages
            .into_iter()
            .map(|message| StreamStep::Item(Ok(message)))
            .collect(),
    )
}

fn valid_remote_error() -> TridentError {
    TridentError {
        kind: TridentErrorKind::InvalidInputError.into(),
        subkind: "bad-config".to_string(),
        message: "configuration is invalid: caused by bad field".to_string(),
        error_message: "bad field".to_string(),
        location: Some(FileLocation {
            path: "src/validation.rs".to_string(),
            line: 42,
        }),
    }
}

#[tokio::test]
async fn endpoint_schemes_support_only_unix_and_http() {
    for endpoint in ["https://127.0.0.1:50051", "ftp://127.0.0.1:50051"] {
        let error = Client::builder()
            .endpoint(endpoint)
            .connect_timeout(SHORT_TIMEOUT)
            .connect()
            .await
            .expect_err("unsupported endpoint scheme should fail");
        assert_eq!(error.execution_state(), ExecutionState::NotStarted);
        assert!(error.to_string().starts_with("invalid Trident endpoint"));
    }

    for endpoint in [
        "unix:///nonexistent/trident-client.sock",
        "http://127.0.0.1:9",
    ] {
        let error = Client::builder()
            .endpoint(endpoint)
            .connect_timeout(SHORT_TIMEOUT)
            .connect()
            .await
            .expect_err("supported endpoint without a server should fail to connect");
        assert_eq!(error.execution_state(), ExecutionState::NotStarted);
        assert!(error.to_string().starts_with("failed to connect"));
    }
}

#[tokio::test]
async fn endpoint_diagnostics_redact_sensitive_data() {
    const ENDPOINT: &str = "ftp://client:password@example.test/socket?token=secret#fragment-secret";
    let builder = Client::builder().endpoint(ENDPOINT);
    let builder_debug = format!("{builder:?}");
    for secret in ["client", "password", "token=secret", "fragment-secret"] {
        assert!(!builder_debug.contains(secret));
    }

    let error = builder.connect().await.unwrap_err();
    assert_eq!(format!("{error:?}"), error.to_string());
    assert!(!format!("{error:?}").contains("InvalidEndpoint"));
    for diagnostic in [error.to_string(), format!("{error:?}")] {
        for secret in ["client", "password", "token=secret", "fragment-secret"] {
            assert!(!diagnostic.contains(secret));
        }
    }

    let malformed = Client::builder()
        .endpoint("not a url?token=secret")
        .connect()
        .await
        .unwrap_err();
    assert_eq!(format!("{malformed:?}"), malformed.to_string());
    for diagnostic in [malformed.to_string(), format!("{malformed:?}")] {
        assert!(!diagnostic.contains("token=secret"));
    }
}

#[tokio::test]
async fn unix_endpoint_decodes_escaped_socket_path() {
    let root = tempfile::tempdir().unwrap();
    let socket_path = root.path().join("trident client.sock");
    let listener = UnixListener::bind(&socket_path).unwrap();
    let mock = MockServer {
        state: Arc::new(MockState::default()),
    };
    tokio::spawn(async move {
        Server::builder()
            .add_service(VersionServiceServer::new(mock))
            .serve_with_incoming(UnixListenerStream::new(listener))
            .await
            .expect("mock Unix server should run");
    });

    let file_url = Url::from_file_path(&socket_path).unwrap();
    let endpoint = format!("unix://{}", file_url.path());
    assert!(endpoint.contains("%20"));

    let client = Client::connect(endpoint).await.unwrap();
    assert_eq!(client.version().await.unwrap(), "test-version");
}

#[tokio::test]
async fn delayed_unary_response_uses_unary_timeout() {
    let state = Arc::new(MockState::default());
    *state.version_delay.lock().unwrap() = Some(LONG_DELAY);
    let client = connect_mock_with_timeouts(state, REQUEST_TIMEOUT, SHORT_TIMEOUT).await;

    let error = client.version().await.unwrap_err();
    assert_eq!(error.execution_state(), ExecutionState::OutcomeUnknown);
    assert_eq!(error.rpc_code(), None);
    assert!(error.to_string().contains("did not complete"));
    assert_eq!(format!("{error:?}"), error.to_string());
    assert!(!format!("{error:?}").contains("UnaryTimeout"));
    assert!(!format!("{error:?}").contains("operation:"));
}

#[tokio::test]
async fn typed_and_raw_update_requests_preserve_contract() {
    let state = Arc::new(MockState::default());
    let client = connect_mock(state.clone(), REQUEST_TIMEOUT).await;
    let config = HostConfiguration::default();
    let expected_typed = serde_yaml::to_string(&config).unwrap();
    let raw = "# compatibility header\nimage: null\n";

    set_action(&state, stream(vec![success()]));
    client
        .update(&config, RebootPolicy::Caller)
        .await
        .unwrap()
        .wait()
        .await
        .unwrap();
    set_action(&state, stream(vec![success()]));
    client
        .update_yaml(raw, RebootPolicy::Trident)
        .await
        .unwrap()
        .wait()
        .await
        .unwrap();

    let captured = state.captured.lock().unwrap();
    assert_eq!(captured.update_yaml, [expected_typed, raw.to_string()]);
    assert_eq!(
        captured.update_reboots,
        [
            RebootHandling::CallerHandlesReboot as i32,
            RebootHandling::TridentHandlesReboot as i32,
        ]
    );
}

#[tokio::test]
async fn stream_disk_maps_structured_url_and_hash() {
    let state = Arc::new(MockState::default());
    let client = connect_mock(state.clone(), REQUEST_TIMEOUT).await;
    let url = Url::parse("https://example.test/image.cosi?channel=stable").unwrap();
    let hash: Sha384Hash =
        serde_yaml::from_value(serde_yaml::Value::String(SHA384.to_string())).unwrap();

    set_action(&state, stream(vec![success()]));
    client
        .stream_disk(&url, Some(&hash), RebootPolicy::Caller)
        .await
        .unwrap()
        .wait()
        .await
        .unwrap();
    set_action(&state, stream(vec![success()]));
    client
        .stream_disk(&url, None, RebootPolicy::Trident)
        .await
        .unwrap()
        .wait()
        .await
        .unwrap();

    let captured = state.captured.lock().unwrap();
    assert_eq!(
        captured.stream_disk[0],
        (
            url.to_string(),
            Some(format!("sha384:{SHA384}")),
            RebootHandling::CallerHandlesReboot as i32,
        )
    );
    assert_eq!(captured.stream_disk[1].1, None);
}

#[tokio::test]
async fn stream_disk_rejects_malformed_hashes_before_rpc() {
    let url = Url::parse("https://example.test/image.cosi").unwrap();
    let cases = [
        Sha384Hash::from("a".repeat(95)),
        Sha384Hash::from(format!("{}g", "a".repeat(95))),
    ];

    for hash in cases {
        let state = Arc::new(MockState::default());
        let client = connect_mock(state.clone(), REQUEST_TIMEOUT).await;
        let error = client
            .stream_disk(&url, Some(&hash), RebootPolicy::Caller)
            .await
            .err()
            .expect("malformed hash should be rejected");
        assert_eq!(error.execution_state(), ExecutionState::NotStarted);
        assert_eq!(error.rpc_code(), None);
        assert!(error.to_string().starts_with("invalid input"));
        assert!(state.captured.lock().unwrap().stream_disk.is_empty());
        assert!(state.actions.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn started_log_and_success_events_decode_to_domain_types() {
    let state = Arc::new(MockState::default());
    let client = connect_mock(state.clone(), REQUEST_TIMEOUT).await;
    let started = ServicingResponse {
        timestamp: Some(Timestamp {
            seconds: 123,
            nanos: 456,
        }),
        response: Some(ResponseBody::Started(Default::default())),
    };
    let log = ServicingResponse {
        timestamp: None,
        response: Some(ResponseBody::Log(Log {
            level: ProtoLogLevel::Warn.into(),
            message: "careful".to_string(),
            module: "engine".to_string(),
            target: "trident".to_string(),
            location: Some(FileLocation {
                path: "src/engine.rs".to_string(),
                line: 8,
            }),
        })),
    };
    let done = completed(Completed {
        status: StatusCode::Success.into(),
        error: None,
        reboot_status: RebootStatus::RebootRequired.into(),
        image_hash: Some(format!("sha384:{SHA384}")),
        servicing_kind: Some(ProtoServicingKind::AbUpdate.into()),
    });
    set_action(&state, stream(vec![started, log, done]));

    let mut events = Vec::new();
    let completion = client
        .update_stage_yaml("{}\n")
        .await
        .unwrap()
        .wait_with(|event| events.push(event.clone()))
        .await
        .unwrap();

    assert_eq!(events.len(), 3);
    assert!(matches!(
        events[0],
        ServicingEvent::Started {
            timestamp: Some(EventTimestamp {
                seconds: 123,
                nanos: 456
            })
        }
    ));
    assert!(matches!(
        &events[1],
        ServicingEvent::Log(LogRecord {
            level: LogLevel::Warn,
            message,
            location: Some(SourceLocation { line: 8, .. }),
            ..
        }) if message == "careful"
    ));
    assert_eq!(completion.reboot, RebootDisposition::Required);
    assert_eq!(completion.servicing_kind, Some(ServicingKind::AbUpdate));
    assert!(matches!(
        completion.image_hash,
        Some(ImageHash::Sha384(ref hash)) if hash.as_str() == SHA384
    ));
}

#[tokio::test]
async fn structured_remote_failure_is_preserved() {
    let state = Arc::new(MockState::default());
    let client = connect_mock(state.clone(), REQUEST_TIMEOUT).await;
    set_action(
        &state,
        stream(vec![completed(Completed {
            status: StatusCode::Failure.into(),
            error: Some(valid_remote_error()),
            reboot_status: RebootStatus::Unspecified.into(),
            image_hash: None,
            servicing_kind: None,
        })]),
    );

    let error = client
        .update_stage_yaml("{}\n")
        .await
        .unwrap()
        .wait()
        .await
        .unwrap_err();
    assert_eq!(error.execution_state(), ExecutionState::Completed);
    let remote = error.remote_error().unwrap();
    assert_eq!(remote.category, RemoteErrorCategory::InvalidInput);
    assert_eq!(remote.subkind, "bad-config");
    assert_eq!(remote.cause(), "bad field");
    assert_eq!(remote.location.as_ref().unwrap().line, 42);
    assert_eq!(remote.to_string(), remote.message);
    let remote_error: &(dyn std::error::Error + 'static) = remote;
    assert_eq!(remote_error.to_string(), remote.message);
    assert_eq!(format!("{error:?}"), error.to_string());
    assert!(!format!("{error:?}").contains("RemoteFailure"));
}

#[tokio::test]
async fn manual_event_consumers_receive_remote_errors_directly() {
    let state = Arc::new(MockState::default());
    let client = connect_mock(state.clone(), REQUEST_TIMEOUT).await;
    set_action(
        &state,
        stream(vec![completed(Completed {
            status: StatusCode::Failure.into(),
            error: Some(valid_remote_error()),
            reboot_status: RebootStatus::Unspecified.into(),
            image_hash: None,
            servicing_kind: None,
        })]),
    );

    let mut operation = client.update_stage_yaml("{}\n").await.unwrap();
    let event = operation.next_event().await.unwrap().unwrap();
    let ServicingEvent::Completed(Err(remote)) = event else {
        panic!("expected failed completion event");
    };
    assert_eq!(remote.category, RemoteErrorCategory::InvalidInput);
    assert_eq!(
        remote.to_string(),
        "configuration is invalid: caused by bad field"
    );
    assert!(operation.next_event().await.unwrap().is_none());
}

#[tokio::test]
async fn protocol_invariants_are_rejected_centrally() {
    let state = Arc::new(MockState::default());
    let client = connect_mock(state.clone(), REQUEST_TIMEOUT).await;
    let cases = [
        ServicingResponse {
            timestamp: None,
            response: None,
        },
        completed(Completed {
            status: StatusCode::Unspecified.into(),
            error: None,
            reboot_status: RebootStatus::RebootNotRequired.into(),
            image_hash: None,
            servicing_kind: None,
        }),
        completed(Completed {
            status: 99,
            error: None,
            reboot_status: RebootStatus::RebootNotRequired.into(),
            image_hash: None,
            servicing_kind: None,
        }),
        completed(Completed {
            status: StatusCode::Success.into(),
            error: Some(valid_remote_error()),
            reboot_status: RebootStatus::RebootNotRequired.into(),
            image_hash: None,
            servicing_kind: None,
        }),
        completed(Completed {
            status: StatusCode::Failure.into(),
            error: None,
            reboot_status: RebootStatus::Unspecified.into(),
            image_hash: None,
            servicing_kind: None,
        }),
        completed(Completed {
            status: StatusCode::Success.into(),
            error: None,
            reboot_status: 99,
            image_hash: None,
            servicing_kind: None,
        }),
    ];

    for response in cases {
        set_action(&state, stream(vec![response]));
        let error = client
            .update_stage_yaml("{}\n")
            .await
            .unwrap()
            .wait()
            .await
            .unwrap_err();
        assert!(error
            .to_string()
            .starts_with("invalid Trident protocol response"));
        assert_eq!(error.remote_error(), None);
        assert_eq!(error.rpc_code(), None);
    }
}

#[tokio::test]
async fn forward_compatible_stream_values_preserve_completion() {
    let log_levels = [
        (ProtoLogLevel::Warn as i32, LogLevel::Warn),
        (ProtoLogLevel::Unspecified as i32, LogLevel::Unspecified),
        (99, LogLevel::Unknown(99)),
    ];

    for (wire, expected) in log_levels {
        let state = Arc::new(MockState::default());
        let client = connect_mock(state.clone(), REQUEST_TIMEOUT).await;
        let log = ServicingResponse {
            timestamp: None,
            response: Some(ResponseBody::Log(Log {
                level: wire,
                message: "forward-compatible".to_string(),
                module: String::new(),
                target: String::new(),
                location: None,
            })),
        };
        set_action(&state, stream(vec![log, success()]));

        let mut observed = None;
        client
            .update_stage_yaml("{}\n")
            .await
            .unwrap()
            .wait_with(|event| {
                if let ServicingEvent::Log(record) = event {
                    observed = Some(record.level);
                }
            })
            .await
            .unwrap();
        assert_eq!(observed, Some(expected));
    }

    let servicing_kinds = [
        (ProtoServicingKind::AbUpdate as i32, ServicingKind::AbUpdate),
        (
            ProtoServicingKind::Unspecified as i32,
            ServicingKind::Unspecified,
        ),
        (99, ServicingKind::Unknown(99)),
    ];
    for (wire, expected) in servicing_kinds {
        let state = Arc::new(MockState::default());
        let client = connect_mock(state.clone(), REQUEST_TIMEOUT).await;
        set_action(
            &state,
            stream(vec![completed(Completed {
                status: StatusCode::Success.into(),
                error: None,
                reboot_status: RebootStatus::RebootNotRequired.into(),
                image_hash: None,
                servicing_kind: Some(wire),
            })]),
        );
        let completion = client
            .update_stage_yaml("{}\n")
            .await
            .unwrap()
            .wait()
            .await
            .unwrap();
        assert_eq!(completion.servicing_kind, Some(expected));
    }
}

#[tokio::test]
async fn forward_compatible_error_categories_and_image_hashes_are_preserved() {
    let categories = [
        (
            TridentErrorKind::InvalidInputError as i32,
            RemoteErrorCategory::InvalidInput,
        ),
        (
            TridentErrorKind::Unspecified as i32,
            RemoteErrorCategory::Unspecified,
        ),
        (99, RemoteErrorCategory::Unknown(99)),
    ];
    for (wire, expected) in categories {
        let state = Arc::new(MockState::default());
        let client = connect_mock(state.clone(), REQUEST_TIMEOUT).await;
        let mut remote = valid_remote_error();
        remote.kind = wire;
        set_action(
            &state,
            stream(vec![completed(Completed {
                status: StatusCode::Failure.into(),
                error: Some(remote),
                reboot_status: RebootStatus::Unspecified.into(),
                image_hash: None,
                servicing_kind: None,
            })]),
        );
        let error = client
            .update_stage_yaml("{}\n")
            .await
            .unwrap()
            .wait()
            .await
            .unwrap_err();
        assert_eq!(error.remote_error().unwrap().category, expected);
    }

    let hashes = [
        (
            format!("sha384:{SHA384}"),
            ImageHash::Sha384(Sha384Hash::from(SHA384)),
        ),
        (
            "sha384:not-a-hash".to_string(),
            ImageHash::Unknown("sha384:not-a-hash".to_string()),
        ),
        (
            "sha512:future".to_string(),
            ImageHash::Unknown("sha512:future".to_string()),
        ),
    ];
    for (wire, expected) in hashes {
        let state = Arc::new(MockState::default());
        let client = connect_mock(state.clone(), REQUEST_TIMEOUT).await;
        set_action(
            &state,
            stream(vec![completed(Completed {
                status: StatusCode::Success.into(),
                error: None,
                reboot_status: RebootStatus::RebootNotRequired.into(),
                image_hash: Some(wire),
                servicing_kind: None,
            })]),
        );
        let completion = client
            .update_stage_yaml("{}\n")
            .await
            .unwrap()
            .wait()
            .await
            .unwrap();
        assert_eq!(completion.image_hash, Some(expected));
    }
}

#[tokio::test]
async fn transport_and_timeout_errors_report_execution_certainty() {
    let state = Arc::new(MockState::default());
    let client = connect_mock(state.clone(), SHORT_TIMEOUT).await;

    set_action(&state, Action::Reject(Status::invalid_argument("rejected")));
    let rejected = client
        .update_stage_yaml("{}\n")
        .await
        .err()
        .expect("request should be rejected");
    assert_eq!(rejected.execution_state(), ExecutionState::NotStarted);
    assert_eq!(rejected.rpc_code(), Some(RpcCode::InvalidArgument));

    set_action(&state, Action::Reject(Status::unavailable("unavailable")));
    let unavailable = client
        .update_stage_yaml("{}\n")
        .await
        .err()
        .expect("request should fail as unavailable");
    assert_eq!(
        unavailable.execution_state(),
        ExecutionState::OutcomeUnknown
    );
    assert_eq!(unavailable.rpc_code(), Some(RpcCode::Unavailable));

    set_action(
        &state,
        Action::DelayStart(LONG_DELAY, vec![StreamStep::Item(Ok(success()))]),
    );
    let acknowledgement_timeout = client
        .update_stage_yaml("{}\n")
        .await
        .err()
        .expect("request acknowledgement should time out");
    assert_eq!(
        acknowledgement_timeout.execution_state(),
        ExecutionState::OutcomeUnknown
    );
    assert_eq!(acknowledgement_timeout.rpc_code(), None);

    set_action(
        &state,
        Action::Stream(vec![
            StreamStep::Item(Ok(ServicingResponse {
                timestamp: None,
                response: Some(ResponseBody::Started(Default::default())),
            })),
            StreamStep::Item(Err(Status::unavailable("connection lost"))),
        ]),
    );
    let stream_error = client
        .update_stage_yaml("{}\n")
        .await
        .unwrap()
        .wait()
        .await
        .unwrap_err();
    assert_eq!(
        stream_error.execution_state(),
        ExecutionState::OutcomeUnknown
    );
    assert_eq!(stream_error.rpc_code(), Some(RpcCode::Unavailable));

    set_action(&state, Action::Stream(Vec::new()));
    let missing = client
        .update_stage_yaml("{}\n")
        .await
        .unwrap()
        .wait()
        .await
        .unwrap_err();
    assert_eq!(missing.execution_state(), ExecutionState::OutcomeUnknown);
    assert_eq!(missing.rpc_code(), None);
    assert!(missing.to_string().contains("ended without completion"));

    set_action(
        &state,
        Action::Stream(vec![
            StreamStep::Item(Ok(ServicingResponse {
                timestamp: None,
                response: Some(ResponseBody::Started(Default::default())),
            })),
            StreamStep::Delay(LONG_DELAY),
            StreamStep::Item(Ok(success())),
        ]),
    );
    let completion_timeout = client
        .update_stage_yaml("{}\n")
        .await
        .unwrap()
        .with_completion_timeout(SHORT_TIMEOUT)
        .wait()
        .await
        .unwrap_err();
    assert_eq!(
        completion_timeout.execution_state(),
        ExecutionState::OutcomeUnknown
    );
    assert_eq!(completion_timeout.rpc_code(), None);
}

#[tokio::test]
async fn stable_rpc_families_map_request_fields() {
    let state = Arc::new(MockState::default());
    let client = connect_mock(state.clone(), REQUEST_TIMEOUT).await;
    let config = HostConfiguration::default();
    let expected_config = serde_yaml::to_string(&config).unwrap();

    assert_eq!(client.version().await.unwrap(), "test-version");

    set_action(&state, stream(vec![success()]));
    client
        .update_stage(&config)
        .await
        .unwrap()
        .wait()
        .await
        .unwrap();
    set_action(&state, stream(vec![success()]));
    client
        .update_finalize(RebootPolicy::Trident)
        .await
        .unwrap()
        .wait()
        .await
        .unwrap();
    set_action(&state, stream(vec![success()]));
    client
        .commit(RebootPolicy::Caller)
        .await
        .unwrap()
        .wait()
        .await
        .unwrap();
    set_action(&state, stream(vec![success()]));
    client
        .rollback(RollbackKind::Runtime, RebootPolicy::Trident)
        .await
        .unwrap()
        .wait()
        .await
        .unwrap();
    set_action(&state, stream(vec![success()]));
    client
        .rollback_stage(RollbackKind::Ab)
        .await
        .unwrap()
        .wait()
        .await
        .unwrap();
    set_action(&state, stream(vec![success()]));
    client
        .rollback_finalize(RebootPolicy::Caller)
        .await
        .unwrap()
        .wait()
        .await
        .unwrap();

    let captured = state.captured.lock().unwrap();
    assert_eq!(captured.version_requests, 1);
    assert!(captured.update_stage_yaml.contains(&expected_config));
    assert_eq!(
        captured.update_finalize_reboots,
        [RebootHandling::TridentHandlesReboot as i32]
    );
    assert_eq!(
        captured.commit_reboots,
        [RebootHandling::CallerHandlesReboot as i32]
    );
    assert_eq!(
        captured.rollbacks,
        [(
            ManualRollbackKind::RuntimeRollbackRequested as i32,
            RebootHandling::TridentHandlesReboot as i32,
        )]
    );
    assert_eq!(
        captured.rollback_stages,
        [ManualRollbackKind::AbRollbackRequested as i32]
    );
    assert_eq!(
        captured.rollback_finalizes,
        [RebootHandling::CallerHandlesReboot as i32]
    );
}

#[cfg(feature = "preview")]
#[tokio::test]
async fn preview_servicing_rpc_families_map_request_fields() {
    let state = Arc::new(MockState::default());
    let client = connect_mock(state.clone(), REQUEST_TIMEOUT).await;
    let config = HostConfiguration::default();
    let expected_typed = serde_yaml::to_string(&config).unwrap();
    let raw = "# installer-owned\n{}\n";

    set_action(&state, stream(vec![success()]));
    client
        .install(&config, RebootPolicy::Caller)
        .await
        .unwrap()
        .wait()
        .await
        .unwrap();
    set_action(&state, stream(vec![success()]));
    client
        .install_yaml(raw, RebootPolicy::Trident)
        .await
        .unwrap()
        .wait()
        .await
        .unwrap();
    set_action(&state, stream(vec![success()]));
    client
        .install_stage(&config)
        .await
        .unwrap()
        .wait()
        .await
        .unwrap();
    set_action(&state, stream(vec![success()]));
    client
        .install_finalize(RebootPolicy::Caller)
        .await
        .unwrap()
        .wait()
        .await
        .unwrap();
    set_action(&state, stream(vec![success()]));
    client.rebuild_raid().await.unwrap().wait().await.unwrap();

    let captured = state.captured.lock().unwrap();
    assert_eq!(
        captured.install_yaml,
        [expected_typed.clone(), raw.to_string()]
    );
    assert_eq!(
        captured.install_reboots,
        [
            RebootHandling::CallerHandlesReboot as i32,
            RebootHandling::TridentHandlesReboot as i32,
        ]
    );
    assert_eq!(captured.install_stage_yaml, [expected_typed]);
    assert_eq!(
        captured.install_finalize_reboots,
        [RebootHandling::CallerHandlesReboot as i32]
    );
    assert_eq!(captured.rebuild_raid_requests, 1);
}

#[cfg(feature = "preview")]
#[tokio::test]
async fn preview_status_validation_and_rollback_use_domain_types() {
    let state = Arc::new(MockState::default());
    let client = connect_mock(state.clone(), REQUEST_TIMEOUT).await;
    let config = HostConfiguration::default();
    *state.servicing_state.lock().unwrap() = ProtoServicingState::UpdateAbStaged.into();
    *state.active_volume.lock().unwrap() = AbVolumeState::VolumeB.into();
    *state.validation_response.lock().unwrap() = ValidateHostConfigurationResponse {
        ok: false,
        error: Some(valid_remote_error()),
    };

    assert_eq!(
        client.servicing_state().await.unwrap(),
        ServicingState::AbUpdateStaged
    );
    assert_eq!(client.active_volume().await.unwrap(), ActiveVolume::VolumeB);
    let raw = "# validation\n{}\n";
    let validation = client.validate_host_configuration_yaml(raw).await.unwrap();
    assert!(!validation.valid);
    assert_eq!(
        validation.error.unwrap().category,
        RemoteErrorCategory::InvalidInput
    );
    assert_eq!(
        state.captured.lock().unwrap().validation_yaml,
        [raw.to_string()]
    );

    *state.validation_response.lock().unwrap() = ValidateHostConfigurationResponse {
        ok: true,
        error: None,
    };
    client.validate_host_configuration(&config).await.unwrap();

    assert_eq!(
        client.check_rollback(RollbackKind::Any).await.unwrap(),
        RollbackAvailability::Ab
    );

    let captured = state.captured.lock().unwrap();
    assert_eq!(captured.servicing_state_requests, 1);
    assert_eq!(captured.active_volume_requests, 1);
    assert_eq!(
        captured.check_rollback_kinds,
        [ManualRollbackKind::AnyRollbackRequested as i32]
    );
    assert_eq!(captured.validation_yaml.len(), 2);
}

#[cfg(feature = "preview")]
#[tokio::test]
async fn preview_status_enums_preserve_known_unspecified_and_unknown_values() {
    let servicing_states = [
        (
            ProtoServicingState::Provisioned as i32,
            ServicingState::Provisioned,
        ),
        (
            ProtoServicingState::Unspecified as i32,
            ServicingState::Unspecified,
        ),
        (99, ServicingState::Unknown(99)),
    ];
    for (wire, expected) in servicing_states {
        let state = Arc::new(MockState::default());
        *state.servicing_state.lock().unwrap() = wire;
        let client = connect_mock(state, REQUEST_TIMEOUT).await;
        assert_eq!(client.servicing_state().await.unwrap(), expected);
    }

    let active_volumes = [
        (AbVolumeState::VolumeA as i32, ActiveVolume::VolumeA),
        (AbVolumeState::Unspecified as i32, ActiveVolume::Unspecified),
        (99, ActiveVolume::Unknown(99)),
    ];
    for (wire, expected) in active_volumes {
        let state = Arc::new(MockState::default());
        *state.active_volume.lock().unwrap() = wire;
        let client = connect_mock(state, REQUEST_TIMEOUT).await;
        assert_eq!(client.active_volume().await.unwrap(), expected);
    }

    let rollback_availability = [
        (
            CheckRollbackKind::RuntimeRollbackExpected as i32,
            RollbackAvailability::Runtime,
        ),
        (
            CheckRollbackKind::Unspecified as i32,
            RollbackAvailability::Unspecified,
        ),
        (99, RollbackAvailability::Unknown(99)),
    ];
    for (wire, expected) in rollback_availability {
        let state = Arc::new(MockState::default());
        *state.check_rollback_kind.lock().unwrap() = wire;
        let client = connect_mock(state, REQUEST_TIMEOUT).await;
        assert_eq!(
            client.check_rollback(RollbackKind::Any).await.unwrap(),
            expected
        );
    }
}

#[tokio::test]
async fn cloned_clients_multiplex_concurrent_requests() {
    let state = Arc::new(MockState::default());
    let client = connect_mock(state.clone(), REQUEST_TIMEOUT).await;
    let clone = client.clone();

    let (first, second) = tokio::join!(client.version(), clone.version());
    assert_eq!(first.unwrap(), "test-version");
    assert_eq!(second.unwrap(), "test-version");
    assert_eq!(state.captured.lock().unwrap().version_requests, 2);
}
