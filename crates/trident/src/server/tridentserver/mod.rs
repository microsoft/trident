use std::{sync::Arc, time::SystemTime};

use log::{error, info, warn};
use prost_types::Timestamp;
use tokio::{
    sync::{
        mpsc::{self, UnboundedSender},
        OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock,
    },
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;
use tonic::{async_trait, Request, Response, Status};

use harpoon::{
    servicing_response::Response as ResponseType, trident_service_server::TridentService,
    CheckRootRequest, CommitRequest, FileLocation, FinalizeRequest, GetActiveVolumeRequest,
    GetActiveVolumeResponse, GetConfigRequest, GetConfigResponse, GetLastErrorRequest,
    GetLastErrorResponse, GetRequiredServicingTypeRequest, GetRequiredServicingTypeResponse,
    GetServicingStateRequest, GetServicingStateResponse, Log, RebuildRaidRequest, ServicingRequest,
    ServicingResponse, StageRequest, Start, StreamImageRequest, ValidateHostConfigurationRequest,
    ValidateHostConfigurationResponse,
};
use trident_api::error::{InternalError, TridentError};

use crate::{
    logging::logfwd::LogForwarder,
    server::{activitytracker::ActivityTracker, support::stream::StreamWithLock},
    ExitKind,
};

mod servicingmgr;

use servicingmgr::ServicingManager;

pub(super) struct TridentHarpoonServer {
    log_forwarder: LogForwarder,
    tracker: ActivityTracker,
    servicing_manager: ServicingManager,
    rwlock: Arc<RwLock<()>>,
}

/// This is the stream type for all servicing responses.
type ServicingResponseStream = StreamWithLock<Result<ServicingResponse, Status>, ()>;

impl TridentHarpoonServer {
    pub(super) fn new(log_forwarder: LogForwarder, tracker: ActivityTracker) -> Self {
        TridentHarpoonServer {
            log_forwarder,
            tracker,
            servicing_manager: ServicingManager::new(),
            rwlock: Arc::new(RwLock::new(())),
        }
    }

    /// Sets up log forwarding from the internal log forwarder to the gRPC
    /// streaming response.
    ///
    /// Internally spawns a background task that listens for log records from
    /// the internal [`LogForwarder`] and sends them over the provided gRPC
    /// channel as [`ServicingResponse`] messages. On success, this function
    /// returns a [`JoinHandle`] for the spawned task together with a
    /// [`CancellationToken`] that can be used by the caller to request
    /// shutdown of the forwarding task.
    ///
    /// # Errors
    ///
    /// Returns an internal gRPC [`Status`] error if the log forwarder sender
    /// channel cannot be installed (i.e. if
    /// [`LogForwarder::set_sender`](crate::LogForwarder::set_sender) returns
    /// an error). In that case, no background task is spawned and no log
    /// records will be forwarded for this stream.
    ///
    /// # Task lifecycle
    ///
    /// The spawned task runs until one of the following occurs:
    /// * The provided cancellation token is cancelled.
    /// * The internal log channel is closed and `recv` returns `None`.
    /// * Sending a log record on `grpc_log_tx` fails.
    ///
    /// When the task terminates for any reason, it attempts to clear the
    /// sender from the internal [`LogForwarder`] so that subsequent streams
    /// can install their own sender if needed.
    fn setup_log_forwarding(
        &self,
        grpc_log_tx: UnboundedSender<Result<ServicingResponse, Status>>,
    ) -> Result<(JoinHandle<()>, CancellationToken), Status> {
        // Set up log forwarding task
        let log_token = CancellationToken::new();
        let (log_tx, mut log_rx) = mpsc::unbounded_channel();

        // Set the sender in the log forwarder
        if self.log_forwarder.set_sender(log_tx).is_err() {
            error!("Failed to set log forwarder sender channel");
            return Err(Status::internal("Failed to set log forwarder"));
        }

        // Spawn log forwarding task
        let log_token_clone = log_token.clone();
        let log_forwarder_clone = self.log_forwarder.clone();
        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = log_token_clone.cancelled() => {
                        break;
                    }

                    channel_msg = log_rx.recv() => {
                        let Some(log_record) = channel_msg else {
                            break;
                        };

                        if let Err(err) = grpc_log_tx.send(Ok(ServicingResponse {
                            timestamp: Some(Timestamp::from(SystemTime::now())),
                            response: Some(ResponseType::Log(Log {
                                message: log_record.message,
                                level: log_record.level as i32,
                                target: log_record.target,
                                module: log_record.module,
                                location: Some(FileLocation {
                                    path: log_record.file,
                                    line: log_record.line,
                                }),
                            })),
                        })) {
                            error!("Failed to send log message in streaming response: {}", err);
                            break;
                        }
                    }
                }
            }

            log_forwarder_clone.clear_sender().unwrap_or_else(|err| {
                error!("Failed to clear log forwarder sender channel: {}", err);
            });
        });

        // Return the handle and cancellation token
        Ok((handle, log_token))
    }

    /// Tries to acquire a read lock on the server's RwLock. If the lock
    /// cannot be acquired, returns a gRPC Status indicating that the server is
    /// busy.
    fn try_acquire_read_lock(&self) -> Result<OwnedRwLockReadGuard<()>, Status> {
        self.rwlock.clone().try_read_owned().map_err(|_| {
            warn!("Trident is busy, cannot acquire read connection lock");
            Status::unavailable("Trident is busy")
        })
    }

    /// Tries to acquire a write lock on the server's RwLock. If the lock
    /// cannot be acquired, returns a gRPC Status indicating that the server is
    /// busy.
    fn try_acquire_write_lock(&self) -> Result<OwnedRwLockWriteGuard<()>, Status> {
        self.rwlock.clone().try_write_owned().map_err(|_| {
            warn!("Trident is busy, cannot acquire write connection lock");
            Status::unavailable("Trident is busy")
        })
    }

    /// Handles a servicing request by acquiring the necessary locks,
    /// setting up log forwarding, and spawning the provided servicing task.
    ///
    /// On success, returns a gRPC streaming response (`Response<ServicingResponseStream>`)
    /// that yields log messages and the final result of the servicing task.
    ///
    /// If the required read/write locks cannot be acquired (for example, when the
    /// server is busy), this returns an error `Status` such as `Status::unavailable`.
    /// It may also return other error `Status` values if log forwarding or task
    /// setup fails. In all error cases, no servicing task is spawned and no stream
    /// of responses is produced.
    fn servicing_request<F>(
        &self,
        name: &'static str,
        f: F,
    ) -> Result<Response<ServicingResponseStream>, Status>
    where
        F: FnOnce() -> Result<ExitKind, TridentError> + Send + 'static,
    {
        info!("Received servicing request '{}'", name);

        // Try to acquire the connection lock in write mode
        let guard = self.try_acquire_write_lock()?;

        // Create the gRPC response channel
        let (tx, rx) = mpsc::unbounded_channel();

        // Try to acquire the servicing lock
        let Some(servicing_guard) = self.servicing_manager.try_lock_servicing() else {
            warn!("Request '{}' blocked because servicing is active", name);
            return Err(Status::unavailable("Servicing is active"));
        };

        // Set up log forwarding. Logs will be sent over the gRPC channel.
        let (log_fwd_handle, log_fwd_token) = self.setup_log_forwarding(tx.clone())?;

        // All prerequisites are met, send start response
        if let Err(err) = tx.send(Ok(ServicingResponse {
            timestamp: Some(Timestamp::from(SystemTime::now())),
            response: Some(ResponseType::Start(Start {})),
        })) {
            error!("Failed to send start response: {}", err);
            return Err(Status::internal("Failed to start processing"));
        }

        // Create a clone of the activity tracker to move into the task
        let tracker_clone = self.tracker.clone();

        // Spawn the servicing task
        tokio::spawn(async move {
            // Spawn the servicing task and await its completion
            let final_status =
                ServicingManager::spawn_servicing_task(servicing_guard, tracker_clone, f).await;

            if let Some(ref err) = final_status.error {
                error!("Servicing request '{}' failed: {}", name, err.message);
            } else {
                info!("Servicing request '{}' completed successfully", name);
            }

            // Stop log forwarding
            log_fwd_token.cancel();

            // Await the log forwarding task to finish to ensure all relevant
            // logs have been sent.
            if let Err(err) = log_fwd_handle.await {
                error!("Log forwarder task failed: {}", err);
            }

            // Send the final status response
            if let Err(err) = tx.send(Ok(ServicingResponse {
                timestamp: Some(Timestamp::from(SystemTime::now())),
                response: Some(ResponseType::FinalStatus(final_status)),
            })) {
                error!("Failed to send control response: {}", err);
            }

            // Close the gRPC channel by dropping the sender. Only two senders
            // exist: this one and the one in the log forwarder, which has
            // already been stopped.
            drop(tx);

            info!("Request '{}' completed", name);
        });

        // Return the streaming response with the lock guard
        Ok(Response::new(StreamWithLock::new(rx, guard)))
    }

    /// Handles a read-only request by acquiring the necessary locks and
    /// executing the provided function.
    ///
    /// This helper:
    /// - Tries to acquire the connection lock in read mode.
    /// - Tries to acquire the servicing read lock, returning
    ///   [`Status::unavailable`] if servicing is currently active.
    /// - Executes the provided function `f` once the locks are held.
    ///
    /// On success, this returns `Ok(Response::new(result))`, where `result` is
    /// the value produced by `f`. If `f` returns an error, the error is logged
    /// and converted into a [`Status::internal`] error. Failures to acquire the
    /// underlying locks are returned as appropriate [`Status`] errors.
    fn reading_request<F, R>(&self, name: &'static str, f: F) -> Result<Response<R>, Status>
    where
        F: FnOnce() -> Result<R, TridentError> + Send + 'static,
        R: Send + 'static,
    {
        info!("Received read request '{}'", name);
        // Try to acquire the connection lock in read mode. We hold a reference
        // to the lock guard to ensure it lives through the duration of the
        // request.
        let _guard = self.try_acquire_read_lock()?;

        // Try to acquire the servicing read lock
        let Some(_servicing_guard) = self.servicing_manager.try_lock_reading() else {
            warn!(
                "Read request '{}' blocked because servicing is active",
                name
            );
            return Err(Status::unavailable("Servicing is active"));
        };

        // Execute the reading function
        // TODO: Execute in a blocking task to avoid blocking the async runtime
        match f() {
            Ok(result) => Ok(Response::new(result)),
            Err(err) => {
                error!("Reading request '{}' failed: {:?}", name, err);
                // TODO: Map specific errors to appropriate Status codes
                Err(Status::internal(format!(
                    "Reading request '{}' failed: {:?}",
                    name, err
                )))
            }
        }
    }
}

/// Implements the gRPC TridentService for the TridentHarpoonServer struct.
#[async_trait]
impl TridentService for TridentHarpoonServer {
    // /// Sample data read method
    // ///
    // /// TODO: Remove once real methods are implemented.
    // async fn read_data(
    //     &self,
    //     _request: Request<ReadRequest>,
    // ) -> Result<Response<ReadResponse>, Status> {
    //     self.reading_request("read_data", || {
    //         let value = servicing::some_reading_operation("hello from server")?;
    //         Ok(ReadResponse { output: value })
    //     })
    // }

    // /// Sample servicing method
    // ///
    // /// TODO: Remove once real methods are implemented.
    // type DoProcessStream = ServicingResponseStream;
    // async fn do_process(
    //     &self,
    //     request: Request<DoProcessRequest>,
    // ) -> Result<Response<Self::DoProcessStream>, Status> {
    //     let process_req = request.into_inner();
    //     self.servicing_request("do_process", move || {
    //         servicing::some_servicing_operation(
    //             process_req.count,
    //             Duration::from_millis(process_req.interval_ms.into()),
    //         )
    //     })
    // }

    type InstallStream = ServicingResponseStream;
    async fn install(
        &self,
        _request: Request<ServicingRequest>,
    ) -> Result<Response<Self::InstallStream>, Status> {
        self.servicing_request("install", || {
            Err(TridentError::new(InternalError::Internal(
                "Not implemented: install",
            )))
        })
    }

    type InstallStageStream = ServicingResponseStream;
    async fn install_stage(
        &self,
        _request: Request<StageRequest>,
    ) -> Result<Response<Self::InstallStageStream>, Status> {
        self.servicing_request("install_stage", || {
            Err(TridentError::new(InternalError::Internal(
                "Not implemented: install_stage",
            )))
        })
    }

    type InstallFinalizeStream = ServicingResponseStream;
    async fn install_finalize(
        &self,
        _request: Request<FinalizeRequest>,
    ) -> Result<Response<Self::InstallFinalizeStream>, Status> {
        self.servicing_request("install_finalize", || {
            Err(TridentError::new(InternalError::Internal(
                "Not implemented: install_finalize",
            )))
        })
    }

    type UpdateStream = ServicingResponseStream;
    async fn update(
        &self,
        _request: Request<ServicingRequest>,
    ) -> Result<Response<Self::UpdateStream>, Status> {
        self.servicing_request("update", || {
            Err(TridentError::new(InternalError::Internal(
                "Not implemented: update",
            )))
        })
    }

    type UpdateStageStream = ServicingResponseStream;
    async fn update_stage(
        &self,
        _request: Request<StageRequest>,
    ) -> Result<Response<Self::UpdateStageStream>, Status> {
        self.servicing_request("update_stage", || {
            Err(TridentError::new(InternalError::Internal(
                "Not implemented: update_stage",
            )))
        })
    }

    type UpdateFinalizeStream = ServicingResponseStream;
    async fn update_finalize(
        &self,
        _request: Request<FinalizeRequest>,
    ) -> Result<Response<Self::UpdateFinalizeStream>, Status> {
        self.servicing_request("update_finalize", || {
            Err(TridentError::new(InternalError::Internal(
                "Not implemented: update_finalize",
            )))
        })
    }

    type CheckRootStream = ServicingResponseStream;
    async fn check_root(
        &self,
        _request: Request<CheckRootRequest>,
    ) -> Result<Response<Self::CheckRootStream>, Status> {
        self.servicing_request("check_root", || {
            Err(TridentError::new(InternalError::Internal(
                "Not implemented: check_root",
            )))
        })
    }

    type CommitStream = ServicingResponseStream;
    async fn commit(
        &self,
        _request: Request<CommitRequest>,
    ) -> Result<Response<Self::CommitStream>, Status> {
        self.servicing_request("commit", || {
            Err(TridentError::new(InternalError::Internal(
                "Not implemented: commit",
            )))
        })
    }

    type StreamImageStream = ServicingResponseStream;
    async fn stream_image(
        &self,
        _request: Request<StreamImageRequest>,
    ) -> Result<Response<Self::StreamImageStream>, Status> {
        self.servicing_request("stream_image", || {
            Err(TridentError::new(InternalError::Internal(
                "Not implemented: stream_image",
            )))
        })
    }

    type RebuildRaidStream = ServicingResponseStream;
    async fn rebuild_raid(
        &self,
        _request: Request<RebuildRaidRequest>,
    ) -> Result<Response<Self::RebuildRaidStream>, Status> {
        self.servicing_request("rebuild_raid", || {
            Err(TridentError::new(InternalError::Internal(
                "Not implemented: rebuild_raid",
            )))
        })
    }

    async fn validate_host_configuration(
        &self,
        _request: Request<ValidateHostConfigurationRequest>,
    ) -> Result<Response<ValidateHostConfigurationResponse>, Status> {
        self.reading_request("validate_host_configuration", || {
            Err(TridentError::new(InternalError::Internal(
                "Not implemented: validate_host_configuration",
            )))
        })
    }

    async fn get_required_servicing_type(
        &self,
        _request: Request<GetRequiredServicingTypeRequest>,
    ) -> Result<Response<GetRequiredServicingTypeResponse>, Status> {
        self.reading_request("get_required_servicing_type", || {
            Err(TridentError::new(InternalError::Internal(
                "Not implemented: get_required_servicing_type",
            )))
        })
    }

    async fn get_provisioned_config(
        &self,
        _request: Request<GetConfigRequest>,
    ) -> Result<Response<GetConfigResponse>, Status> {
        self.reading_request("get_provisioned_config", || {
            Err(TridentError::new(InternalError::Internal(
                "Not implemented: get_provisioned_config",
            )))
        })
    }

    async fn get_servicing_config(
        &self,
        _request: Request<GetConfigRequest>,
    ) -> Result<Response<GetConfigResponse>, Status> {
        self.reading_request("get_servicing_config", || {
            Err(TridentError::new(InternalError::Internal(
                "Not implemented: get_servicing_config",
            )))
        })
    }

    async fn get_last_error(
        &self,
        _request: Request<GetLastErrorRequest>,
    ) -> Result<Response<GetLastErrorResponse>, Status> {
        self.reading_request("get_last_error", || {
            Err(TridentError::new(InternalError::Internal(
                "Not implemented: get_last_error",
            )))
        })
    }

    async fn get_servicing_state(
        &self,
        _request: Request<GetServicingStateRequest>,
    ) -> Result<Response<GetServicingStateResponse>, Status> {
        self.reading_request("get_servicing_state", || {
            Err(TridentError::new(InternalError::Internal(
                "Not implemented: get_servicing_state",
            )))
        })
    }

    async fn get_active_volume(
        &self,
        _request: Request<GetActiveVolumeRequest>,
    ) -> Result<Response<GetActiveVolumeResponse>, Status> {
        self.reading_request("get_active_volume", || {
            Err(TridentError::new(InternalError::Internal(
                "Not implemented: get_active_volume",
            )))
        })
    }
}
