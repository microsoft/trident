use std::{
    fmt,
    future::Future,
    path::{Path, PathBuf},
    time::Duration,
};

use hyper_util::rt::TokioIo;
use tokio::{net::UnixStream, time};
use tonic::{
    transport::{Channel, Endpoint, Uri},
    Request, Response, Status, Streaming,
};
use tower::service_fn;

use trident_api::{config::HostConfiguration, primitives::hash::Sha384Hash};
use trident_proto::{
    v1::{
        commit_service_client::CommitServiceClient, rollback_service_client::RollbackServiceClient,
        streaming_service_client::StreamingServiceClient,
        update_service_client::UpdateServiceClient, version_service_client::VersionServiceClient,
        CommitRequest, FinalizeUpdateRequest, HostConfiguration as ProtoHostConfiguration,
        ManualRollbackKind, RebootHandling, RebootManagement, RollbackFinalizeRequest,
        RollbackRequest, RollbackStageRequest, ServicingResponse, StageUpdateRequest,
        StreamDiskRequest, UpdateRequest, VersionRequest,
    },
    TRIDENT_DEFAULT_SOCKET_URI,
};
use url::Url;

use crate::{types::Operation, ClientError, RebootPolicy, RollbackKind, ServicingOperation};

/// Default `tridentd` Unix socket URI.
pub const DEFAULT_ENDPOINT: &str = TRIDENT_DEFAULT_SOCKET_URI;

const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_REQUEST_ACKNOWLEDGEMENT_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_UNARY_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECTOR_ENDPOINT: &str = "http://[::]:50051";
const SHA384_HEX_LENGTH: usize = 96;

/// Builder for a multiplexed [`Client`].
#[derive(Clone)]
pub struct ClientBuilder {
    endpoint: String,
    connect_timeout: Duration,
    request_acknowledgement_timeout: Duration,
    unary_timeout: Duration,
}

impl fmt::Debug for ClientBuilder {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ClientBuilder")
            .field(
                "endpoint",
                &endpoint_for_diagnostics(self.endpoint.as_str()),
            )
            .field("connect_timeout", &self.connect_timeout)
            .field(
                "request_acknowledgement_timeout",
                &self.request_acknowledgement_timeout,
            )
            .field("unary_timeout", &self.unary_timeout)
            .finish()
    }
}

impl Default for ClientBuilder {
    fn default() -> Self {
        Self {
            endpoint: DEFAULT_ENDPOINT.to_string(),
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            request_acknowledgement_timeout: DEFAULT_REQUEST_ACKNOWLEDGEMENT_TIMEOUT,
            unary_timeout: DEFAULT_UNARY_TIMEOUT,
        }
    }
}

impl ClientBuilder {
    /// Overrides the server endpoint.
    pub fn endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = endpoint.into();
        self
    }

    /// Sets the connection timeout.
    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// Sets the maximum time to wait for an initial RPC acknowledgement.
    pub fn request_acknowledgement_timeout(mut self, timeout: Duration) -> Self {
        self.request_acknowledgement_timeout = timeout;
        self
    }

    /// Sets the maximum time to wait for a unary RPC response.
    pub fn unary_timeout(mut self, timeout: Duration) -> Self {
        self.unary_timeout = timeout;
        self
    }

    /// Establishes the shared HTTP/2 channel.
    pub async fn connect(self) -> Result<Client, ClientError> {
        let channel = connect_channel(&self.endpoint, self.connect_timeout).await?;
        Ok(Client {
            channel,
            request_acknowledgement_timeout: self.request_acknowledgement_timeout,
            unary_timeout: self.unary_timeout,
        })
    }
}

/// High-level, cloneable client for `tridentd`.
#[derive(Clone, Debug)]
pub struct Client {
    pub(crate) channel: Channel,
    request_acknowledgement_timeout: Duration,
    unary_timeout: Duration,
}

impl Client {
    /// Creates a client builder using the default Unix socket.
    pub fn builder() -> ClientBuilder {
        ClientBuilder::default()
    }

    /// Connects to an explicit endpoint with default timeouts.
    pub async fn connect(endpoint: impl Into<String>) -> Result<Self, ClientError> {
        Self::builder().endpoint(endpoint).connect().await
    }

    /// Connects to [`DEFAULT_ENDPOINT`] with default timeouts.
    pub async fn connect_default() -> Result<Self, ClientError> {
        Self::builder().connect().await
    }

    #[cfg(test)]
    pub(crate) fn from_channel(
        channel: Channel,
        request_acknowledgement_timeout: Duration,
        unary_timeout: Duration,
    ) -> Self {
        Self {
            channel,
            request_acknowledgement_timeout,
            unary_timeout,
        }
    }

    /// Returns the `tridentd` version.
    pub async fn version(&self) -> Result<String, ClientError> {
        let mut client = VersionServiceClient::new(self.channel.clone());
        let response = self
            .unary(
                Operation::Version,
                client.version(Request::new(VersionRequest {})),
            )
            .await?;
        Ok(response.into_inner().version)
    }

    /// Streams an image directly to disk.
    pub async fn stream_disk(
        &self,
        image_url: &Url,
        image_hash: Option<&Sha384Hash>,
        reboot: RebootPolicy,
    ) -> Result<ServicingOperation, ClientError> {
        if let Some(image_hash) = image_hash {
            validate_sha384_hash(image_hash)?;
        }
        let request = StreamDiskRequest {
            image_url: image_url.to_string(),
            image_hash: image_hash.map(|hash| format!("sha384:{hash}")),
            reboot: Some(reboot_management(reboot)),
        };
        let mut client = StreamingServiceClient::new(self.channel.clone());
        self.servicing(
            Operation::StreamDisk,
            client.stream_disk(Request::new(request)),
        )
        .await
    }

    /// Performs an update from a typed Host Configuration.
    pub async fn update(
        &self,
        config: &HostConfiguration,
        reboot: RebootPolicy,
    ) -> Result<ServicingOperation, ClientError> {
        self.update_yaml(serialize_config(Operation::Update, config)?, reboot)
            .await
    }

    /// Performs an update using Host Configuration YAML unchanged.
    pub async fn update_yaml(
        &self,
        config: impl Into<String>,
        reboot: RebootPolicy,
    ) -> Result<ServicingOperation, ClientError> {
        let request = UpdateRequest {
            stage: Some(StageUpdateRequest {
                config: Some(proto_config(config)),
            }),
            finalize: Some(FinalizeUpdateRequest {
                reboot: Some(reboot_management(reboot)),
            }),
        };
        let mut client = UpdateServiceClient::new(self.channel.clone());
        self.servicing(Operation::Update, client.update(Request::new(request)))
            .await
    }

    /// Stages an update from a typed Host Configuration.
    pub async fn update_stage(
        &self,
        config: &HostConfiguration,
    ) -> Result<ServicingOperation, ClientError> {
        self.update_stage_yaml(serialize_config(Operation::UpdateStage, config)?)
            .await
    }

    /// Stages an update using Host Configuration YAML unchanged.
    pub async fn update_stage_yaml(
        &self,
        config: impl Into<String>,
    ) -> Result<ServicingOperation, ClientError> {
        let request = StageUpdateRequest {
            config: Some(proto_config(config)),
        };
        let mut client = UpdateServiceClient::new(self.channel.clone());
        self.servicing(
            Operation::UpdateStage,
            client.update_stage(Request::new(request)),
        )
        .await
    }

    /// Finalizes a staged update.
    pub async fn update_finalize(
        &self,
        reboot: RebootPolicy,
    ) -> Result<ServicingOperation, ClientError> {
        let request = FinalizeUpdateRequest {
            reboot: Some(reboot_management(reboot)),
        };
        let mut client = UpdateServiceClient::new(self.channel.clone());
        self.servicing(
            Operation::UpdateFinalize,
            client.update_finalize(Request::new(request)),
        )
        .await
    }

    /// Commits completed servicing.
    pub async fn commit(&self, reboot: RebootPolicy) -> Result<ServicingOperation, ClientError> {
        let request = CommitRequest {
            reboot: Some(reboot_management(reboot)),
        };
        let mut client = CommitServiceClient::new(self.channel.clone());
        self.servicing(Operation::Commit, client.commit(Request::new(request)))
            .await
    }

    /// Performs a full rollback.
    pub async fn rollback(
        &self,
        kind: RollbackKind,
        reboot: RebootPolicy,
    ) -> Result<ServicingOperation, ClientError> {
        let request = RollbackRequest {
            stage: Some(RollbackStageRequest {
                kind: rollback_kind(kind),
            }),
            finalize: Some(RollbackFinalizeRequest {
                reboot: Some(reboot_management(reboot)),
            }),
        };
        let mut client = RollbackServiceClient::new(self.channel.clone());
        self.servicing(Operation::Rollback, client.rollback(Request::new(request)))
            .await
    }

    /// Stages a rollback.
    pub async fn rollback_stage(
        &self,
        kind: RollbackKind,
    ) -> Result<ServicingOperation, ClientError> {
        let request = RollbackStageRequest {
            kind: rollback_kind(kind),
        };
        let mut client = RollbackServiceClient::new(self.channel.clone());
        self.servicing(
            Operation::RollbackStage,
            client.rollback_stage(Request::new(request)),
        )
        .await
    }

    /// Finalizes a staged rollback.
    pub async fn rollback_finalize(
        &self,
        reboot: RebootPolicy,
    ) -> Result<ServicingOperation, ClientError> {
        let request = RollbackFinalizeRequest {
            reboot: Some(reboot_management(reboot)),
        };
        let mut client = RollbackServiceClient::new(self.channel.clone());
        self.servicing(
            Operation::RollbackFinalize,
            client.rollback_finalize(Request::new(request)),
        )
        .await
    }

    pub(crate) async fn servicing<F>(
        &self,
        operation: Operation,
        request: F,
    ) -> Result<ServicingOperation, ClientError>
    where
        F: FutureServicingResponse,
    {
        let response = time::timeout(self.request_acknowledgement_timeout, request.into_future())
            .await
            .map_err(|_| {
                ClientError::request_acknowledgement_timeout(
                    operation,
                    self.request_acknowledgement_timeout,
                )
            })?
            .map_err(|status| ClientError::request_rejected(operation, status))?;
        Ok(ServicingOperation::new(operation, response.into_inner()))
    }

    pub(crate) async fn unary<T, F>(
        &self,
        operation: Operation,
        request: F,
    ) -> Result<Response<T>, ClientError>
    where
        F: FutureUnaryResponse<T>,
    {
        time::timeout(self.unary_timeout, request.into_future())
            .await
            .map_err(|_| ClientError::unary_timeout(operation, self.unary_timeout))?
            .map_err(|status| ClientError::request_rejected(operation, status))
    }
}

pub(crate) trait FutureServicingResponse {
    type Future: Future<Output = Result<Response<Streaming<ServicingResponse>>, Status>> + Send;

    fn into_future(self) -> Self::Future;
}

impl<F> FutureServicingResponse for F
where
    F: Future<Output = Result<Response<Streaming<ServicingResponse>>, Status>> + Send,
{
    type Future = F;

    fn into_future(self) -> Self::Future {
        self
    }
}

pub(crate) trait FutureUnaryResponse<T> {
    type Future: Future<Output = Result<Response<T>, Status>> + Send;

    fn into_future(self) -> Self::Future;
}

impl<T, F> FutureUnaryResponse<T> for F
where
    F: Future<Output = Result<Response<T>, Status>> + Send,
{
    type Future = F;

    fn into_future(self) -> Self::Future {
        self
    }
}

async fn connect_channel(endpoint: &str, timeout: Duration) -> Result<Channel, ClientError> {
    let url =
        Url::parse(endpoint).map_err(|error| invalid_endpoint(endpoint, error.to_string()))?;

    match url.scheme() {
        "unix" => connect_unix(endpoint, &url, timeout).await,
        "http" => connect_tcp(endpoint, timeout).await,
        scheme => Err(invalid_endpoint(
            endpoint,
            format!("unsupported endpoint scheme '{scheme}'"),
        )),
    }
}

async fn connect_tcp(endpoint: &str, timeout: Duration) -> Result<Channel, ClientError> {
    let transport = Endpoint::from_shared(endpoint.to_string())
        .map_err(|error| invalid_endpoint(endpoint, error.to_string()))?;
    time::timeout(timeout, transport.connect())
        .await
        .map_err(|_| connection_error(endpoint, format!("connection timed out after {timeout:?}")))?
        .map_err(|error| connection_error(endpoint, error.to_string()))
}

async fn connect_unix(
    endpoint: &str,
    url: &Url,
    timeout: Duration,
) -> Result<Channel, ClientError> {
    if url.host().is_some() || url.query().is_some() || url.fragment().is_some() {
        return Err(invalid_endpoint(
            endpoint,
            "Unix endpoints must contain only an absolute socket path",
        ));
    }
    let path = unix_socket_path(endpoint, url)?;
    if !path.is_absolute() || path == Path::new("/") {
        return Err(invalid_endpoint(
            endpoint,
            "Unix endpoint path must be absolute and non-empty",
        ));
    }

    let transport = Endpoint::from_shared(CONNECTOR_ENDPOINT.to_string())
        .map_err(|error| invalid_endpoint(endpoint, error.to_string()))?;
    let connector = service_fn(move |_: Uri| {
        let path = path.clone();
        async move { UnixStream::connect(path).await.map(TokioIo::new) }
    });
    time::timeout(timeout, transport.connect_with_connector(connector))
        .await
        .map_err(|_| connection_error(endpoint, format!("connection timed out after {timeout:?}")))?
        .map_err(|error| connection_error(endpoint, error.to_string()))
}

fn unix_socket_path(endpoint: &str, url: &Url) -> Result<PathBuf, ClientError> {
    let file_url = Url::parse(&format!("file://{}", url.path())).map_err(|error| {
        invalid_endpoint(
            endpoint,
            format!("Unix endpoint path could not be decoded: {error}"),
        )
    })?;
    file_url
        .to_file_path()
        .map_err(|()| invalid_endpoint(endpoint, "Unix endpoint path could not be decoded"))
}

fn invalid_endpoint(endpoint: &str, details: impl Into<String>) -> ClientError {
    ClientError::invalid_endpoint(
        endpoint_for_diagnostics(endpoint),
        endpoint_details_for_diagnostics(endpoint, details),
    )
}

fn connection_error(endpoint: &str, details: impl Into<String>) -> ClientError {
    ClientError::connection(
        endpoint_for_diagnostics(endpoint),
        endpoint_details_for_diagnostics(endpoint, details),
    )
}

fn endpoint_for_diagnostics(endpoint: &str) -> String {
    let Ok(mut url) = Url::parse(endpoint) else {
        return "<invalid endpoint>".to_string();
    };

    if !url.username().is_empty() && url.set_username("redacted").is_err() {
        return "<redacted endpoint>".to_string();
    }
    if url.password().is_some() && url.set_password(Some("redacted")).is_err() {
        return "<redacted endpoint>".to_string();
    }
    if url.query().is_some() {
        url.set_query(Some("redacted"));
    }
    if url.fragment().is_some() {
        url.set_fragment(Some("redacted"));
    }

    url.to_string()
}

fn endpoint_details_for_diagnostics(endpoint: &str, details: impl Into<String>) -> String {
    match Url::parse(endpoint) {
        Ok(url)
            if !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some() =>
        {
            "details redacted because the endpoint contains sensitive data".to_string()
        }
        _ => details.into(),
    }
}

pub(crate) fn serialize_config(
    operation: Operation,
    config: &HostConfiguration,
) -> Result<String, ClientError> {
    serde_yaml::to_string(config)
        .map_err(|error| ClientError::serialization(operation, error.to_string()))
}

pub(crate) fn proto_config(config: impl Into<String>) -> ProtoHostConfiguration {
    ProtoHostConfiguration {
        config: config.into(),
    }
}

pub(crate) fn reboot_management(policy: RebootPolicy) -> RebootManagement {
    let handling = match policy {
        RebootPolicy::Trident => RebootHandling::TridentHandlesReboot,
        RebootPolicy::Caller => RebootHandling::CallerHandlesReboot,
    };
    RebootManagement {
        handling: handling.into(),
    }
}

pub(crate) fn rollback_kind(kind: RollbackKind) -> i32 {
    match kind {
        RollbackKind::Any => ManualRollbackKind::AnyRollbackRequested,
        RollbackKind::Ab => ManualRollbackKind::AbRollbackRequested,
        RollbackKind::Runtime => ManualRollbackKind::RuntimeRollbackRequested,
    }
    .into()
}

fn validate_sha384_hash(hash: &Sha384Hash) -> Result<(), ClientError> {
    let value = hash.as_str();
    if value.len() != SHA384_HEX_LENGTH {
        return Err(ClientError::invalid_input(
            Operation::StreamDisk,
            format!(
                "SHA-384 hash has length {}; expected {SHA384_HEX_LENGTH} hexadecimal characters",
                value.len()
            ),
        ));
    }
    if !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ClientError::invalid_input(
            Operation::StreamDisk,
            "SHA-384 hash contains non-hexadecimal characters",
        ));
    }
    Ok(())
}
