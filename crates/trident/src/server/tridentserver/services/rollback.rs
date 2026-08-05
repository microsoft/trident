use tonic::{async_trait, Request, Response, Status};

use trident_api::{
    config::{Operation, Operations},
    error::{TridentError, TridentResultExt},
};
use trident_proto::v1::{
    rollback_service_server::RollbackService, ManualRollbackKind, RollbackFinalizeRequest,
    RollbackRequest, RollbackStageRequest,
};

#[cfg(feature = "grpc-preview")]
use trident_api::error::InternalError;
#[cfg(feature = "grpc-preview")]
use trident_proto::v1preview::{
    rollback_service_server::RollbackService as RollbackServicePreview, CheckRollbackKind,
    CheckRollbackRequest, CheckRollbackResponse, GetRollbackChainRequest, GetRollbackChainResponse,
    GetRollbackTargetRequest, GetRollbackTargetResponse,
};

#[cfg(feature = "grpc-preview")]
use crate::engine::manual_rollback::utils::{
    ManualRollbackContext, ManualRollbackKind as InternalManualRollbackKind,
    ManualRollbackRequestKind,
};
use crate::{
    server::{
        tridentserver::{RebootDecision, ServicingResponseStream},
        TridentServer,
    },
    DataStore, Trident,
};

/// Converts a wire-level `ManualRollbackKind` into the internal
/// `ManualRollbackRequestKind`. Only used by the preview-only
/// `check_rollback` query below.
#[cfg(feature = "grpc-preview")]
fn manual_rollback_request_kind(
    kind: ManualRollbackKind,
) -> Result<ManualRollbackRequestKind, TridentError> {
    match kind {
        ManualRollbackKind::Unspecified | ManualRollbackKind::AnyRollbackRequested => {
            Ok(ManualRollbackRequestKind::RollbackNext)
        }
        ManualRollbackKind::AbRollbackRequested => {
            Ok(ManualRollbackRequestKind::RollbackAvailableAbUpdate)
        }
        ManualRollbackKind::RuntimeRollbackRequested => {
            Ok(ManualRollbackRequestKind::RollbackOnlyIfNextIsRuntimeUpdate)
        }
    }
}

#[async_trait]
impl RollbackService for TridentServer {
    type RollbackStream = ServicingResponseStream;
    async fn rollback(
        &self,
        request: Request<RollbackRequest>,
    ) -> Result<Response<Self::RollbackStream>, Status> {
        let req = request.into_inner();
        let Some(finalize) = req.finalize else {
            return Err(Status::invalid_argument("Missing finalize configuration"));
        };

        let data_store_path = self.agent_config.datastore_path().to_owned();
        let logstream = self.logstream.clone();
        let tracestream = self.tracestream.clone();

        self.servicing_request(
            "rollback",
            super::reboot_allowed(&finalize.reboot),
            move || {
                let mut trident: Trident =
                    Trident::new(None, &data_store_path, logstream, tracestream)
                        .message("Failed to initialize Trident")?;

                let mut datastore = DataStore::open_or_create(&data_store_path)
                    .message("Failed to open datastore")?;

                let (invoke_if_next_is_runtime, invoke_available_ab) =
                    manual_rollback_flags(req.kind())?;

                trident
                    .rollback(
                        &mut datastore,
                        invoke_if_next_is_runtime,
                        invoke_available_ab,
                        Operations::all(),
                    )
                    .map(|(exit_kind, servicing_type)| {
                        (exit_kind, None, Some(servicing_type.into()))
                    })
            },
        )
    }

    type RollbackStageStream = ServicingResponseStream;
    async fn rollback_stage(
        &self,
        request: Request<RollbackStageRequest>,
    ) -> Result<Response<Self::RollbackStageStream>, Status> {
        let req = request.into_inner();

        let data_store_path = self.agent_config.datastore_path().to_owned();
        let logstream = self.logstream.clone();
        let tracestream = self.tracestream.clone();

        self.servicing_request("rollback_stage", RebootDecision::Error, move || {
            let mut trident: Trident = Trident::new(None, &data_store_path, logstream, tracestream)
                .message("Failed to initialize Trident")?;

            let mut datastore =
                DataStore::open_or_create(&data_store_path).message("Failed to open datastore")?;

            let (invoke_if_next_is_runtime, invoke_available_ab) =
                manual_rollback_flags(req.kind())?;

            trident
                .rollback(
                    &mut datastore,
                    invoke_if_next_is_runtime,
                    invoke_available_ab,
                    Operation::Stage.into(),
                )
                .map(|(exit_kind, servicing_type)| (exit_kind, None, Some(servicing_type.into())))
        })
    }

    type RollbackFinalizeStream = ServicingResponseStream;
    async fn rollback_finalize(
        &self,
        request: Request<RollbackFinalizeRequest>,
    ) -> Result<Response<Self::RollbackFinalizeStream>, Status> {
        let finalize = request.into_inner();

        let data_store_path = self.agent_config.datastore_path().to_owned();
        let logstream = self.logstream.clone();
        let tracestream = self.tracestream.clone();

        self.servicing_request(
            "rollback_finalize",
            super::reboot_allowed(&finalize.reboot),
            move || {
                let mut trident: Trident =
                    Trident::new(None, &data_store_path, logstream, tracestream)
                        .message("Failed to initialize Trident")?;

                let mut datastore = DataStore::open_or_create(&data_store_path)
                    .message("Failed to open datastore")?;

                // The rollback kind was already resolved and staged by
                // RollbackStage/Rollback, so finalize just needs to proceed
                // with whatever is currently staged. RollbackNext resolves to
                // the currently-staged rollback because staging already
                // narrowed the chain.
                trident
                    .rollback(&mut datastore, false, false, Operation::Finalize.into())
                    .map(|(exit_kind, servicing_type)| {
                        (exit_kind, None, Some(servicing_type.into()))
                    })
            },
        )
    }
}

/// Converts a wire-level `ManualRollbackKind` into the
/// `(invoke_if_next_is_runtime, invoke_available_ab)` flag pair expected by
/// `Trident::rollback`.
fn manual_rollback_flags(kind: ManualRollbackKind) -> Result<(bool, bool), TridentError> {
    match kind {
        ManualRollbackKind::Unspecified | ManualRollbackKind::AnyRollbackRequested => {
            Ok((false, false))
        }
        ManualRollbackKind::AbRollbackRequested => Ok((false, true)),
        ManualRollbackKind::RuntimeRollbackRequested => Ok((true, false)),
    }
}

#[cfg(feature = "grpc-preview")]
#[async_trait]
impl RollbackServicePreview for TridentServer {
    async fn check_rollback(
        &self,
        request: Request<CheckRollbackRequest>,
    ) -> Result<Response<CheckRollbackResponse>, Status> {
        let req = request.into_inner();
        let data_store_path = self.agent_config.datastore_path().to_owned();

        self.reading_request("check_rollback", move || {
            let requested_kind = manual_rollback_request_kind(req.kind())?;

            let datastore =
                DataStore::open_or_create(&data_store_path).message("Failed to open datastore")?;

            let host_statuses = datastore
                .get_host_statuses()
                .message("Failed to get datastore HostStatus entries")?;
            let rollback_context = ManualRollbackContext::new(&host_statuses)
                .message("Failed to create manual rollback context")?;
            let kind = rollback_context.get_requested_rollback(requested_kind)?;

            Ok(CheckRollbackResponse {
                kind: match kind.map(|item| item.kind) {
                    None => CheckRollbackKind::NoRollbackAvailable,
                    Some(InternalManualRollbackKind::Ab) => CheckRollbackKind::AbRollbackExpected,
                    Some(InternalManualRollbackKind::Runtime) => {
                        CheckRollbackKind::RuntimeRollbackExpected
                    }
                }
                .into(),
            })
        })
        .await
    }

    async fn get_rollback_chain(
        &self,
        _request: Request<GetRollbackChainRequest>,
    ) -> Result<Response<GetRollbackChainResponse>, Status> {
        self.reading_request("get_rollback_chain", || {
            Err(TridentError::new(InternalError::Internal(
                "Not implemented: get_rollback_chain",
            )))
        })
        .await
    }

    async fn get_rollback_target(
        &self,
        _request: Request<GetRollbackTargetRequest>,
    ) -> Result<Response<GetRollbackTargetResponse>, Status> {
        self.reading_request("get_rollback_target", || {
            Err(TridentError::new(InternalError::Internal(
                "Not implemented: get_rollback_target",
            )))
        })
        .await
    }
}
