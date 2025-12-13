use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

use anyhow::Error;
use prost_types::Timestamp;
use tokio::{
    sync::{
        mpsc::{self, UnboundedSender},
        OwnedRwLockWriteGuard, RwLock,
    },
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;
use tonic::{async_trait, Request, Response, Status};

use crate::{
    // TODO: Enable once #396 is closed.
    // app_proto::{
    //     DoProcessRequest, Log, ReadRequest, ReadResponse, ServicingResponse, Start,
    //     app_service_server::AppService, servicing_response::Body,
    // },
    logging::logfwd::LogForwarder,
    server::{activitytracker::ActivityTracker, support::stream::StreamWithLock},
};

mod servicingmgr;

use servicingmgr::ServicingManager;

pub(super) struct AppServer {
    log_forwarder: LogForwarder,
    tracker: ActivityTracker,
    servicing_manager: ServicingManager,
    rwlock: Arc<RwLock<u32>>,
}

// TODO: Enable once #396 is closed.
// /// This is the stream type for all servicing responses.
// type ServicingResponseStream = StreamWithLock<Result<ServicingResponse, Status>, u32>;

impl AppServer {
    pub(super) fn new(log_forwarder: LogForwarder, tracker: ActivityTracker) -> Self {
        AppServer {
            log_forwarder,
            tracker,
            servicing_manager: ServicingManager::new(),
            rwlock: Arc::new(RwLock::new(0)),
        }
    }

    /* TODO: Enable once #396 is closed.
    /// Sets up log forwarding from the internal log forwarder to the gRPC
    /// streaming response. Internally spawns a task that listens for log records
    /// and sends them over the provided gRPC channel.
    fn setup_log_forwarding(
        &self,
        grpc_log_tx: UnboundedSender<Result<ServicingResponse, Status>>,
    ) -> Result<(JoinHandle<()>, CancellationToken), Status> {
        // Set up log forwarding task
        let log_token = CancellationToken::new();
        let (log_tx, mut log_rx) = mpsc::unbounded_channel();

        // Set the sender in the log forwarder
        if let Err(_) = self.log_forwarder.set_sender(log_tx) {
            log::error!("Failed to set log forwarder sender channel");
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

                        let log_message = format!(
                            "[{}][{}][{}] {}",
                            log_record.level,
                            log_record.timestamp.to_rfc3339(),
                            log_record.module,
                            log_record.message,
                        );

                        if let Err(err) = grpc_log_tx.send(Ok(ServicingResponse {
                            timestamp: Some(Timestamp::from(SystemTime::now())),
                            body: Some(Body::Log(Log {
                                message: log_message,
                            })),
                        })) {
                            log::error!("Failed to send log message in streaming response: {}", err);
                            break;
                        }
                    }
                }
            }

            log_forwarder_clone.clear_sender().unwrap_or_else(|err| {
                log::error!("Failed to clear log forwarder sender channel: {}", err);
            });
        });

        // Return the handle and cancellation token
        Ok((handle, log_token))
    }
    */

    /// Tries to acquire a read lock on the server's RwLock. If the lock
    /// cannot be acquired, returns a gRPC Status indicating that the server is
    /// busy.
    fn try_acquire_read_lock(&self) -> Result<OwnedRwLockWriteGuard<u32>, Status> {
        self.rwlock.clone().try_write_owned().map_err(|_| {
            log::warn!("Trident is busy, cannot acquire read lock");
            Status::unavailable("Trident is busy")
        })
    }

    /// Tries to acquire a write lock on the server's RwLock. If the lock
    /// cannot be acquired, returns a gRPC Status indicating that the server is
    /// busy.
    fn try_acquire_write_lock(&self) -> Result<OwnedRwLockWriteGuard<u32>, Status> {
        self.rwlock.clone().try_write_owned().map_err(|_| {
            log::warn!("Trident is busy, cannot acquire write lock");
            Status::unavailable("Trident is busy")
        })
    }

    /* TODO: Enable once #396 is closed.
    /// Handles a servicing request by acquiring the necessary locks,
    /// setting up log forwarding, and spawning the provided servicing task.
    fn servicing_request<F>(
        &self,
        name: &'static str,
        f: F,
    ) -> Result<Response<ServicingResponseStream>, Status>
    where
        F: FnOnce() -> Result<(), Error> + Send + 'static,
    {
        log::info!("Received servicing request '{}'", name);
        let guard = self.try_acquire_write_lock()?;
        let (tx, rx) = mpsc::unbounded_channel();
        let (log_fwd_handle, log_fwd_token) = self.setup_log_forwarding(tx.clone())?;
        if let Err(err) = tx.send(Ok(ServicingResponse {
            timestamp: Some(Timestamp::from(SystemTime::now())),
            body: Some(Body::Start(Start {})),
        })) {
            log::error!("Failed to send start response: {}", err);
            return Err(Status::internal("Failed to start processing"));
        }

        let Some(servicing_guard) = self.servicing_manager.try_lock_servicing() else {
            log::warn!("Request '{}' blocked because servicing is active", name);
            return Err(Status::unavailable("Servicing is active"));
        };

        let tracker_clone = self.tracker.clone();
        tokio::spawn(async move {
            let control =
                ServicingManager::spawn_servicing_task(servicing_guard, tracker_clone, f).await;

            if let Err(err) = tx.send(Ok(ServicingResponse {
                timestamp: Some(Timestamp::from(SystemTime::now())),
                body: Some(Body::Control(control)),
            })) {
                log::error!("Failed to send control response: {}", err);
            }

            log_fwd_token.cancel();
            if let Err(err) = log_fwd_handle.await {
                log::error!("Log forwarder task failed: {}", err);
            }
            log::info!("Request '{}' completed", name);
        });

        Ok(Response::new(StreamWithLock::new(rx, guard)))
    }
    */

    /// Handles a reading request by acquiring the necessary locks and
    /// executing the provided function.
    fn reading_request<F, R>(&self, name: &'static str, f: F) -> Result<Response<R>, Status>
    where
        F: FnOnce() -> Result<R, Error> + Send + 'static,
        R: Send + 'static,
    {
        let _guard = self.try_acquire_read_lock()?;
        let Some(_servicing_guard) = self.servicing_manager.try_lock_reading() else {
            log::warn!("read_data request blocked because servicing is active");
            return Err(Status::unavailable("Servicing is active"));
        };

        match f() {
            Ok(result) => Ok(Response::new(result)),
            Err(err) => {
                log::error!("Reading request failed: {}", err);
                Err(Status::internal(format!("Reading request failed: {}", err)))
            }
        }
    }
}

/* TODO: Enable once #396 is closed.
/// Implements the gRPC AppService for the Trident server.
#[async_trait]
impl AppService for AppServer {
    async fn read_data(
        &self,
        _request: Request<ReadRequest>,
    ) -> Result<Response<ReadResponse>, Status> {
        self.reading_request("read_data", || {
            let value = servicing::some_reading_operation("hello from server")?;
            Ok(ReadResponse { output: value })
        })
    }

    type DoProcessStream = ServicingResponseStream;
    async fn do_process(
        &self,
        request: Request<DoProcessRequest>,
    ) -> Result<Response<Self::DoProcessStream>, Status> {
        let process_req = request.into_inner();
        self.servicing_request("do_process", move || {
            servicing::some_servicing_operation(
                process_req.count,
                Duration::from_millis(process_req.interval_ms.into()),
            )
        })
    }
}
*/
