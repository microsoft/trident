use super::*;

use std::{
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

use trident_api::{config::HostConfiguration, primitives::hash::Sha384Hash, status::ServicingType};

#[cfg(feature = "preview")]
use trident_api::status::ServicingState;
use trident_proto::v1::{
    rollback_service_server::{RollbackService, RollbackServiceServer},
    servicing_response::Response as ResponseBody,
    streaming_service_server::{StreamingService, StreamingServiceServer},
    update_service_server::{UpdateService, UpdateServiceServer},
    version_service_server::{VersionService, VersionServiceServer},
    Completed, FileLocation, FinalizeUpdateRequest, Log, LogLevel as ProtoLogLevel,
    ManualRollbackKind, RebootHandling, RebootStatus, RollbackFinalizeRequest, RollbackRequest,
    RollbackStageRequest, ServicingKind, ServicingResponse, StageUpdateRequest, StatusCode,
    StreamDiskRequest, TridentError, TridentErrorKind, UpdateRequest, VersionRequest,
    VersionResponse,
};

#[cfg(feature = "preview")]
use trident_proto::{
    v1::HostConfiguration as ProtoHostConfiguration,
    v1preview::{
        install_service_server::{InstallService, InstallServiceServer},
        status_service_server::{StatusService, StatusServiceServer},
        validation_service_server::{ValidationService, ValidationServiceServer},
        AbVolumeState, FinalizeInstallRequest, GetActiveVolumeRequest, GetActiveVolumeResponse,
        GetConfigRequest, GetConfigResponse, GetLastErrorRequest, GetLastErrorResponse,
        GetRequiredServicingTypeRequest, GetRequiredServicingTypeResponse,
        GetServicingStateRequest, GetServicingStateResponse, InstallRequest,
        ServicingState as ProtoServicingState, ServicingType as ProtoServicingType,
        StageInstallRequest, ValidateHostConfigurationRequest, ValidateHostConfigurationResponse,
    },
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
    update_yaml: Vec<String>,
    update_reboots: Vec<i32>,
    update_stage_yaml: Vec<String>,
    stream_disk: Vec<(String, Option<String>, i32)>,
    rollback_stages: Vec<i32>,
    rollback_finalizes: Vec<i32>,
    #[cfg(feature = "preview")]
    install_yaml: Vec<String>,
    #[cfg(feature = "preview")]
    install_reboots: Vec<i32>,
    #[cfg(feature = "preview")]
    validation_yaml: Vec<String>,
}

struct MockState {
    action: Mutex<Option<Action>>,
    captured: Mutex<Captured>,
    version_delay: Mutex<Option<Duration>>,
    #[cfg(feature = "preview")]
    status_config_yaml: Mutex<Option<String>>,
    #[cfg(feature = "preview")]
    servicing_state: Mutex<i32>,
    #[cfg(feature = "preview")]
    validation_response: Mutex<ValidateHostConfigurationResponse>,
}

impl Default for MockState {
    fn default() -> Self {
        Self {
            action: Mutex::new(None),
            captured: Mutex::new(Captured::default()),
            version_delay: Mutex::new(None),
            #[cfg(feature = "preview")]
            status_config_yaml: Mutex::new(None),
            #[cfg(feature = "preview")]
            servicing_state: Mutex::new(ProtoServicingState::Provisioned.into()),
            #[cfg(feature = "preview")]
            validation_response: Mutex::new(ValidateHostConfigurationResponse {
                ok: true,
                error: None,
            }),
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
            .action
            .lock()
            .unwrap()
            .take()
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
        _request: Request<FinalizeUpdateRequest>,
    ) -> Result<Response<Self::UpdateFinalizeStream>, Status> {
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
        _request: Request<RollbackRequest>,
    ) -> Result<Response<Self::RollbackStream>, Status> {
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
        _request: Request<StageInstallRequest>,
    ) -> Result<Response<Self::InstallStageStream>, Status> {
        self.respond().await
    }

    async fn install_finalize(
        &self,
        _request: Request<FinalizeInstallRequest>,
    ) -> Result<Response<Self::InstallFinalizeStream>, Status> {
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
        let config = self
            .state
            .status_config_yaml
            .lock()
            .unwrap()
            .clone()
            .map(|config| ProtoHostConfiguration { config });
        Ok(Response::new(GetConfigResponse { config }))
    }

    async fn get_servicing_config(
        &self,
        request: Request<GetConfigRequest>,
    ) -> Result<Response<GetConfigResponse>, Status> {
        self.get_provisioned_config(request).await
    }

    async fn get_last_error(
        &self,
        _request: Request<GetLastErrorRequest>,
    ) -> Result<Response<GetLastErrorResponse>, Status> {
        Ok(Response::new(GetLastErrorResponse { error: None }))
    }

    async fn get_servicing_state(
        &self,
        _request: Request<GetServicingStateRequest>,
    ) -> Result<Response<GetServicingStateResponse>, Status> {
        Ok(Response::new(GetServicingStateResponse {
            state: *self.state.servicing_state.lock().unwrap(),
        }))
    }

    async fn get_active_volume(
        &self,
        _request: Request<GetActiveVolumeRequest>,
    ) -> Result<Response<GetActiveVolumeResponse>, Status> {
        Ok(Response::new(GetActiveVolumeResponse {
            active_volume: AbVolumeState::VolumeA.into(),
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
        Ok(Response::new(GetRequiredServicingTypeResponse {
            servicing_type: ProtoServicingType::AbUpdate.into(),
        }))
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
            .add_service(RollbackServiceServer::new(mock.clone()));
        #[cfg(feature = "preview")]
        let server = server
            .add_service(InstallServiceServer::new(mock.clone()))
            .add_service(StatusServiceServer::new(mock.clone()))
            .add_service(ValidationServiceServer::new(mock));
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
    *state.action.lock().unwrap() = Some(action);
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
        assert!(matches!(error, ClientError::InvalidEndpoint { .. }));
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
        assert!(matches!(error, ClientError::Connection { .. }));
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
    assert!(matches!(
        &error,
        ClientError::UnaryTimeout {
            operation: Operation::Version,
            ..
        }
    ));
    assert_eq!(error.execution_state(), ExecutionState::OutcomeUnknown);
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
        servicing_kind: Some(ServicingKind::AbUpdate.into()),
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
    assert_eq!(completion.servicing_type, Some(ServicingType::AbUpdate));
    assert_eq!(completion.image_hash.as_ref().unwrap().as_str(), SHA384);
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
            reboot_status: RebootStatus::RebootNotRequired.into(),
            image_hash: Some("sha384:not-a-hash".to_string()),
            servicing_kind: None,
        }),
        completed(Completed {
            status: StatusCode::Success.into(),
            error: None,
            reboot_status: RebootStatus::RebootNotRequired.into(),
            image_hash: None,
            servicing_kind: Some(ServicingKind::Unspecified.into()),
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
        assert!(matches!(error, ClientError::InvalidProtocol { .. }));
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
    assert!(matches!(
        rejected,
        ClientError::RequestRejected {
            code: RpcCode::InvalidArgument,
            ..
        }
    ));

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
    assert!(matches!(
        unavailable,
        ClientError::RequestRejected {
            code: RpcCode::Unavailable,
            ..
        }
    ));

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
    assert!(matches!(
        acknowledgement_timeout,
        ClientError::RequestAcknowledgementTimeout { .. }
    ));

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
    assert!(matches!(stream_error, ClientError::StreamFailure { .. }));

    set_action(&state, Action::Stream(Vec::new()));
    let missing = client
        .update_stage_yaml("{}\n")
        .await
        .unwrap()
        .wait()
        .await
        .unwrap_err();
    assert_eq!(missing.execution_state(), ExecutionState::OutcomeUnknown);
    assert!(matches!(missing, ClientError::MissingCompletion { .. }));

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
    assert!(matches!(
        completion_timeout,
        ClientError::CompletionTimeout { .. }
    ));
}

#[tokio::test]
async fn stable_rollback_stage_and_finalize_map_requests() {
    let state = Arc::new(MockState::default());
    let client = connect_mock(state.clone(), REQUEST_TIMEOUT).await;
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
async fn preview_install_typed_and_raw_requests_have_parity() {
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

    let captured = state.captured.lock().unwrap();
    assert_eq!(captured.install_yaml, [expected_typed, raw.to_string()]);
    assert_eq!(
        captured.install_reboots,
        [
            RebootHandling::CallerHandlesReboot as i32,
            RebootHandling::TridentHandlesReboot as i32,
        ]
    );
}

#[cfg(feature = "preview")]
#[tokio::test]
async fn preview_status_config_state_and_validation_use_domain_types() {
    let state = Arc::new(MockState::default());
    let client = connect_mock(state.clone(), REQUEST_TIMEOUT).await;
    let config = HostConfiguration::default();
    let future_yaml = "# future daemon field\nfutureField: true\n";
    *state.status_config_yaml.lock().unwrap() = Some(future_yaml.to_string());
    *state.servicing_state.lock().unwrap() = ProtoServicingState::UpdateAbStaged.into();
    *state.validation_response.lock().unwrap() = ValidateHostConfigurationResponse {
        ok: false,
        error: Some(valid_remote_error()),
    };

    assert_eq!(
        client.provisioned_config_yaml().await.unwrap().as_deref(),
        Some(future_yaml)
    );
    assert_eq!(
        client.servicing_config_yaml().await.unwrap().as_deref(),
        Some(future_yaml)
    );
    let error = client.provisioned_config().await.unwrap_err();
    assert!(matches!(error, ClientError::Deserialization { .. }));

    *state.status_config_yaml.lock().unwrap() = Some(serde_yaml::to_string(&config).unwrap());
    assert_eq!(client.provisioned_config().await.unwrap(), Some(config));
    assert_eq!(
        client.servicing_state().await.unwrap(),
        ServicingState::AbUpdateStaged
    );
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
    assert_eq!(
        client.required_servicing_type_yaml("{}\n").await.unwrap(),
        ServicingType::AbUpdate
    );
}

#[test]
fn public_domain_types_do_not_require_transport_types() {
    fn accepts_domain(
        _: Operation,
        _: RebootPolicy,
        _: RollbackKind,
        _: ExecutionState,
        _: RpcCode,
    ) {
    }

    accepts_domain(
        Operation::Update,
        RebootPolicy::Caller,
        RollbackKind::Ab,
        ExecutionState::OutcomeUnknown,
        RpcCode::Unavailable,
    );
}
