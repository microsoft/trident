use std::{fmt, future::Future, time::Duration};

use tokio::time;
use tonic::{transport::Channel, Request, Response, Status, Streaming};

use trident_api::{config::HostConfiguration, primitives::hash::Sha384Hash};
use trident_proto::{
    v1::{
        commit_service_client::CommitServiceClient, rollback_service_client::RollbackServiceClient,
        streaming_service_client::StreamingServiceClient,
        update_service_client::UpdateServiceClient, version_service_client::VersionServiceClient,
        CommitRequest, FinalizeUpdateRequest, RollbackFinalizeRequest, RollbackRequest,
        RollbackStageRequest, ServicingResponse, StageUpdateRequest, StreamDiskRequest,
        UpdateRequest, VersionRequest,
    },
    TRIDENT_DEFAULT_SOCKET_URI,
};
use url::Url;

use crate::{
    protocol::{self, Operation},
    transport, ClientError, RebootPolicy, RollbackKind, ServicingOperation,
};

/// Default `tridentd` Unix socket URI.
pub const DEFAULT_ENDPOINT: &str = TRIDENT_DEFAULT_SOCKET_URI;

const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_REQUEST_ACKNOWLEDGEMENT_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_UNARY_TIMEOUT: Duration = Duration::from_secs(30);

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
                &transport::endpoint_for_diagnostics(self.endpoint.as_str()),
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
        let channel = transport::connect_channel(&self.endpoint, self.connect_timeout).await?;
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
            protocol::validate_sha384_hash(image_hash)?;
        }
        let request = StreamDiskRequest {
            image_url: image_url.to_string(),
            image_hash: image_hash.map(|hash| format!("sha384:{hash}")),
            reboot: Some(protocol::reboot_management(reboot)),
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
        self.update_yaml(
            protocol::serialize_config(Operation::Update, config)?,
            reboot,
        )
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
                config: Some(protocol::proto_config(config)),
            }),
            finalize: Some(FinalizeUpdateRequest {
                reboot: Some(protocol::reboot_management(reboot)),
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
        self.update_stage_yaml(protocol::serialize_config(Operation::UpdateStage, config)?)
            .await
    }

    /// Stages an update using Host Configuration YAML unchanged.
    pub async fn update_stage_yaml(
        &self,
        config: impl Into<String>,
    ) -> Result<ServicingOperation, ClientError> {
        let request = StageUpdateRequest {
            config: Some(protocol::proto_config(config)),
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
            reboot: Some(protocol::reboot_management(reboot)),
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
            reboot: Some(protocol::reboot_management(reboot)),
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
                kind: protocol::rollback_kind(kind),
            }),
            finalize: Some(RollbackFinalizeRequest {
                reboot: Some(protocol::reboot_management(reboot)),
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
            kind: protocol::rollback_kind(kind),
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
            reboot: Some(protocol::reboot_management(reboot)),
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
        F: Future<Output = Result<Response<Streaming<ServicingResponse>>, Status>> + Send,
    {
        let response = time::timeout(self.request_acknowledgement_timeout, request)
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
        F: Future<Output = Result<Response<T>, Status>> + Send,
    {
        time::timeout(self.unary_timeout, request)
            .await
            .map_err(|_| ClientError::unary_timeout(operation, self.unary_timeout))?
            .map_err(|status| ClientError::request_rejected(operation, status))
    }
}
