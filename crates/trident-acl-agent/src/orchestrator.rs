//! The Trident ACL agent's reconcile loop: watches the Node's request
//! annotation, drives Trident (stage/finalize/rollback/commit) over gRPC,
//! and writes the status annotation back, including post-reboot.
//!
//! Implements the node-side control flow from `docs/update-trigger-design.md`:
//! https://msazure.visualstudio.com/One/_git/Compute-ACL-Update-Service?version=GC1cfe79ec53bfc6936771e2433cba3dec0906b4fd&path=/docs/update-trigger-design.md
//! (sections 2.1 "Trigger mechanism", 2.3 "Stage/finalize/rollback split
//! and post-reboot commit", and 2.5 "Rollback"). See that document for the
//! full state-machine rationale; keep it in sync with this file if the
//! design changes.

use std::{collections::BTreeMap, future::Future};

use anyhow::Context;
use chrono::{DateTime, Utc};
use futures::StreamExt;
use k8s_openapi::api::core::v1::Node;
use semver::Version;
use trident_proto::v1::{RebootStatus, ServicingKind};
use url::Url;
use uuid::Uuid;

use osutils::dependencies::Dependency;

use crate::{
    annotations::{
        current_active_version, Operation, RequestedOperation, StatusCode, UpdateRequest,
        UpdateStatus, SCHEMA_VERSION, UPDATE_COMMIT_STATUS_ANNOTATION, UPDATE_REQUEST_ANNOTATION,
        UPDATE_STATUS_ANNOTATION,
    },
    config::AgentConfig,
    k8s::{K8sClientError, NodeClient},
    nebraska::{CheckOutcome, Client as NebraskaClient, ProgressEvent},
    state::{PendingCommit, StateStore},
    trident::{CompletedResponse, TridentClient, TridentClientError},
    IdSource,
};

const FINAL_STATUS_PATCH_RETRIES: usize = 3;
const FINAL_STATUS_PATCH_BACKOFF: std::time::Duration = std::time::Duration::from_secs(2);

/// The machine-id source used for every Nebraska request this module makes,
/// event reports included. Must match the source used by `handle_stage`'s
/// initial `check_for_update` so all requests for a given node present the
/// same instance identity to Nebraska.
const NEBRASKA_MACHINE_ID_SOURCE: IdSource = IdSource::MachineIdHashed;

/// A single Nebraska event report to send, decoupled from the async
/// machinery that sends it (see `Orchestrator::report_nebraska_event`) so
/// the "what to report" decision can be made by plain, unit-testable
/// functions (`stage_nebraska_report`, `finalize_nebraska_report`,
/// `commit_nebraska_report` below).
#[derive(Debug, Clone, PartialEq, Eq)]
enum NebraskaReport {
    /// An in-flight progress event; see `nebraska::ProgressEvent`. Sending
    /// one commits the instance to eventually reporting a terminal event
    /// (`Completed` or `Failed`) too.
    Progress {
        version: Version,
        event: ProgressEvent,
    },
    /// The terminal "success" event, sent after a reboot onto the new
    /// version.
    Completed { previous: Version, current: Version },
    /// The terminal "failure" event. Clears Nebraska's `update_in_progress`
    /// for the instance so a later check can grant an update again -
    /// required to avoid permanently wedging the instance after a progress
    /// event was already sent.
    Failed { previous: Version, current: Version },
}

impl NebraskaReport {
    /// A short label for logging.
    fn label(&self) -> &'static str {
        match self {
            NebraskaReport::Progress { event, .. } => event.label(),
            NebraskaReport::Completed { .. } => "completed",
            NebraskaReport::Failed { .. } => "failed",
        }
    }
}

#[derive(Clone, Default)]
pub struct SystemRebooter;

pub trait RebootHandle: Clone + Send + Sync + 'static {
    fn reboot(&self) -> Result<(), anyhow::Error>;
}

impl RebootHandle for SystemRebooter {
    fn reboot(&self) -> Result<(), anyhow::Error> {
        // Route through the repo's centralized dependency runner so a
        // missing systemctl binary or non-zero exit produces the same
        // uniform, actionable error type used everywhere else in the
        // codebase (see crates/trident/src/reboot.rs for the same pattern).
        Dependency::Systemctl
            .cmd()
            .arg("reboot")
            .run_and_check()
            .map_err(|err| anyhow::anyhow!("failed to issue systemctl reboot: {err}"))
    }
}

pub struct Orchestrator<R = SystemRebooter> {
    config: AgentConfig,
    k8s: NodeClient,
    rebooter: R,
    state: StateStore,
}

impl Orchestrator<SystemRebooter> {
    pub async fn from_config(config: AgentConfig) -> Result<Self, anyhow::Error> {
        let k8s = NodeClient::new(&config.kubernetes).await?;
        Ok(Self {
            state: StateStore::new(config.orchestration.state_path.clone()),
            config,
            k8s,
            rebooter: SystemRebooter,
        })
    }
}

impl<R> Orchestrator<R>
where
    R: RebootHandle,
{
    pub async fn run(&self) -> Result<(), anyhow::Error> {
        if let Err(err) = self.recover_from_trident_state().await {
            if self.log_and_swallow_node_gone(&err, "recovering persisted state") {
                return Ok(());
            }
            return Err(err);
        }
        let mut stream = self
            .k8s
            .watch_node(self.config.kubernetes.node_name.clone());
        while let Some(node) = stream.next().await {
            let node = node?;
            match self.reconcile_node(&node).await {
                Ok(LoopControl::Continue) => {}
                Ok(LoopControl::ExitForReboot) => return Ok(()),
                Err(err) => {
                    if self.log_and_swallow_node_gone(&err, "reconciling node") {
                        return Ok(());
                    }
                    return Err(err);
                }
            }
        }
        Ok(())
    }

    async fn recover_from_trident_state(&self) -> Result<(), anyhow::Error> {
        let node = self.k8s.get_node(&self.config.kubernetes.node_name).await?;
        let snapshot = Snapshot::from_node(&node);
        let persisted = self.state.load()?;

        if let Some(pending) = persisted.pending_commit.clone() {
            return self.resume_pending_commit(pending).await;
        }

        if let Some(request) = snapshot.request.clone() {
            if let Some(entry) = persisted.completed.get(&request.operation_id) {
                if let Some(commit) = entry.commit.clone() {
                    let matches = snapshot
                        .commit_status
                        .as_ref()
                        .is_some_and(|current| current.same_content(&commit));
                    if !matches {
                        self.publish_status(&commit).await?;
                    }
                }
                if let Some(operation) = entry.operation.clone() {
                    let matches = snapshot
                        .operation_status
                        .as_ref()
                        .is_some_and(|current| current.same_content(&operation));
                    if !matches {
                        self.publish_status(&operation).await?;
                    }
                    return Ok(());
                }
            }
        }

        if let Some(request) = snapshot.request {
            if matches!(
                request.operation,
                RequestedOperation::Finalize | RequestedOperation::Rollback
            ) {
                let status = self.reconstruct_without_state(&request, None, None).await;
                self.record_and_publish(status).await?;
            }
        }
        Ok(())
    }

    async fn reconcile_node(&self, node: &Node) -> Result<LoopControl, anyhow::Error> {
        let snapshot = Snapshot::from_node(node);
        log::debug!(
            "received node update: request={:?} operation_status={:?} commit_status={:?}",
            snapshot.request,
            snapshot.operation_status,
            snapshot.commit_status
        );
        let persisted = self.state.load()?;

        if let Some(invalid) = snapshot.invalid_request.clone() {
            // Dedupe the same way the completed-status cache above does:
            // only publish once per operationId, so a persistently invalid
            // annotation doesn't re-PATCH on every reconcile.
            if !persisted.completed.contains_key(&invalid.operation_id) {
                let now = Utc::now();
                let status = UpdateStatus {
                    schema_version: SCHEMA_VERSION.to_string(),
                    node_update_id: invalid.node_update_id,
                    operation_id: invalid.operation_id,
                    operation: invalid.operation,
                    code: StatusCode::InvalidRequest,
                    message: invalid.reason,
                    from_version: None,
                    to_version: None,
                    started_utc: now,
                    last_updated_utc: now,
                    finished_utc: Some(now),
                };
                self.record_and_publish(status).await?;
            }
            return Ok(LoopControl::Continue);
        }

        let Some(request) = snapshot.request.clone() else {
            return Ok(LoopControl::Continue);
        };

        let cached = persisted
            .completed
            .get(&request.operation_id)
            .and_then(|entry| entry.operation.clone());
        if let Some(status) = cached {
            let matches = snapshot
                .operation_status
                .as_ref()
                .is_some_and(|current| current.same_content(&status));
            if !matches {
                self.publish_status(&status).await?;
            }
            return Ok(LoopControl::Continue);
        }

        if let Some(pending) = persisted.pending_commit.as_ref() {
            // Reject on operationId, not nodeUpdateId: the actual conflict
            // this guard exists to prevent is "a second finalize/rollback
            // starts while one is still waiting for its post-reboot
            // commit" (accepted-design-v2.md's in-flight conflict rule).
            // Keying on nodeUpdateId alone let a retried/re-issued request
            // that reused the same nodeUpdateId but a new operationId slip
            // through this guard entirely and re-enter handle_finalize/
            // handle_rollback concurrently with the still-outstanding
            // original operation.
            if request.operation_id != pending.request.operation_id {
                let started = Utc::now();
                let status = UpdateStatus::new(
                    &request,
                    request.operation.into(),
                    request.operation_id.clone(),
                    StatusCode::InvalidRequest,
                    format!(
                        "another finalize/rollback (operationId {}) is waiting for post-reboot commit",
                        pending.request.operation_id
                    ),
                    pending.from_version.clone(),
                    pending.to_version.clone(),
                    started,
                    Some(Utc::now()),
                );
                self.record_and_publish(status).await?;
                return Ok(LoopControl::Continue);
            }
        }

        match request.operation {
            RequestedOperation::Stage => {
                self.handle_stage(request).await?;
                Ok(LoopControl::Continue)
            }
            RequestedOperation::Finalize => self.handle_finalize(request).await,
            RequestedOperation::Rollback => self.handle_rollback(request).await,
        }
    }

    /// Resolves which Nebraska endpoint to use for `request`: the request
    /// annotation's own `server` override, if present, otherwise the
    /// agent's configured `TRIDENT_ACL_AGENT_NEBRASKA_ENDPOINT` (or CLI
    /// override). Every Nebraska call this `nodeUpdateId` makes - stage's
    /// update check plus every progress/completion event report - must go
    /// through this resolver rather than reading `self.config.nebraska.endpoint`
    /// directly, since Nebraska's per-instance state is tied to one specific
    /// server: mixing endpoints across one update's lifecycle would split
    /// that state across two servers.
    fn resolve_nebraska_endpoint(&self, request: &UpdateRequest) -> Option<Url> {
        request
            .server
            .clone()
            .or_else(|| self.config.nebraska.endpoint.clone())
    }

    /// Resolves which Nebraska app id to use for `request`: the request
    /// annotation's own `appId` override, if present, otherwise the agent's
    /// configured `TRIDENT_ACL_AGENT_NEBRASKA_APP_ID`. Unlike
    /// [`resolve_nebraska_endpoint`], this always resolves to a value -
    /// `TRIDENT_ACL_AGENT_NEBRASKA_APP_ID` always has one (defaulting to
    /// [`crate::DEFAULT_NEBRASKA_APP_ID`]) - so there is no error case to
    /// handle at call sites.
    fn resolve_nebraska_app_id(&self, request: &UpdateRequest) -> String {
        request
            .app_id
            .clone()
            .unwrap_or_else(|| self.config.nebraska.app_id.clone())
    }

    /// Resolves which Nebraska track to use for `request`: the request
    /// annotation's own `track` override, if present, otherwise the agent's
    /// configured `TRIDENT_ACL_AGENT_NEBRASKA_TRACK`. Same always-resolves
    /// behavior as [`resolve_nebraska_app_id`] -
    /// `TRIDENT_ACL_AGENT_NEBRASKA_TRACK` always has a default
    /// ([`crate::DEFAULT_NEBRASKA_TRACK`]) - so there is no error case here
    /// either.
    fn resolve_nebraska_track(&self, request: &UpdateRequest) -> String {
        request
            .track
            .clone()
            .unwrap_or_else(|| self.config.nebraska.track.clone())
    }

    async fn handle_stage(&self, request: UpdateRequest) -> Result<(), anyhow::Error> {
        let started = Utc::now();
        let from_version = Some(current_active_version());
        let to_version = request.target_version.clone();
        if from_version == to_version {
            let status = UpdateStatus::new(
                &request,
                Operation::Stage,
                request.operation_id.clone(),
                StatusCode::AlreadyAtTarget,
                "node already running requested target version",
                from_version,
                to_version,
                started,
                Some(Utc::now()),
            );
            self.record_and_publish(status).await?;
            return Ok(());
        }

        let in_progress = UpdateStatus::new(
            &request,
            Operation::Stage,
            request.operation_id.clone(),
            StatusCode::InProgress,
            "staging update",
            from_version.clone(),
            to_version.clone(),
            started,
            None,
        );
        self.publish_status(&in_progress).await?;
        let endpoint = self.resolve_nebraska_endpoint(&request).ok_or_else(|| {
            anyhow::anyhow!(
                "annotation mode requires request.server, TRIDENT_ACL_AGENT_NEBRASKA_ENDPOINT, or CLI override"
            )
        })?;
        let app_id = self.resolve_nebraska_app_id(&request);
        let track = self.resolve_nebraska_track(&request);
        let machine_id = crate::build_machine_id(IdSource::MachineIdHashed)?;
        let outcome = tokio::task::spawn_blocking(move || {
            let client = NebraskaClient::new(endpoint, app_id, track, machine_id);
            client.check_for_update(&Version::new(0, 0, 0))
        })
        .await
        .context("Nebraska query task panicked")?
        .map_err(|err| anyhow::anyhow!("Nebraska query failed: {err}"))?;
        let offered = match outcome {
            CheckOutcome::UpToDate | CheckOutcome::UpdateInProgress => {
                let status = UpdateStatus::new(
                    &request,
                    Operation::Stage,
                    request.operation_id.clone(),
                    StatusCode::OperationFailed,
                    "Nebraska currently offers no update for the requested target",
                    from_version,
                    to_version,
                    started,
                    Some(Utc::now()),
                );
                self.record_and_publish(status).await?;
                return Ok(());
            }
            CheckOutcome::UpdateAvailable(offer) => offer,
        };
        if request.target_version.as_deref() != Some(offered.version.to_string().as_str()) {
            let status = UpdateStatus::new(
                &request,
                Operation::Stage,
                request.operation_id.clone(),
                StatusCode::InvalidRequest,
                format!(
                    "requested target version {:?} but Nebraska offers {}",
                    request.target_version, offered.version
                ),
                from_version,
                to_version,
                started,
                Some(Utc::now()),
            );
            self.record_and_publish(status).await?;
            return Ok(());
        }

        let current_ver = parse_nebraska_version(&from_version, "stage");
        if let Some(ref v) = current_ver {
            self.report_nebraska_event(
                &request,
                NebraskaReport::Progress {
                    version: v.clone(),
                    event: ProgressEvent::DownloadStarted,
                },
            )
            .await;
        }

        let mut client = TridentClient::connect(&self.config.trident.socket).await?;
        // Integrity of the downloaded image is verified by Trident itself
        // via the image's own COSI metadata, so the Nebraska-reported hash
        // (offered.primary.hash) is not passed here.
        let result = self
            .run_with_status_heartbeat(
                in_progress,
                client.update_stage(
                    &offered.primary.url,
                    None,
                    self.config.orchestration.stage_timeout,
                ),
            )
            .await;
        if let Some(ref v) = current_ver {
            self.report_nebraska_event(&request, stage_nebraska_report(v, &result))
                .await;
        }
        let status = stage_result_to_status(&request, from_version, to_version, started, result);
        self.record_and_publish(status).await
    }

    async fn handle_finalize(&self, request: UpdateRequest) -> Result<LoopControl, anyhow::Error> {
        let started = Utc::now();
        let from_version = Some(current_active_version());
        let to_version = request.target_version.clone();
        if from_version == to_version {
            let status = UpdateStatus::new(
                &request,
                Operation::Finalize,
                request.operation_id.clone(),
                StatusCode::AlreadyAtTarget,
                "node already running requested target version",
                from_version,
                to_version,
                started,
                Some(Utc::now()),
            );
            self.record_and_publish(status).await?;
            return Ok(LoopControl::Continue);
        }

        let completed = self.state.load()?.completed;
        let staged = completed
            .values()
            .filter_map(|entry| entry.operation.as_ref())
            .find(|status| {
                status.node_update_id == request.node_update_id
                    && status.operation == Operation::Stage
                    && matches!(
                        status.code,
                        StatusCode::Success | StatusCode::AlreadyAtTarget
                    )
            });
        let staged = match staged {
            None => {
                let status = UpdateStatus::new(
                    &request,
                    Operation::Finalize,
                    request.operation_id.clone(),
                    StatusCode::NotStaged,
                    "finalize requested without prior successful stage for nodeUpdateId",
                    from_version,
                    to_version,
                    started,
                    Some(Utc::now()),
                );
                self.record_and_publish(status).await?;
                return Ok(LoopControl::Continue);
            }
            Some(staged) => staged,
        };
        if staged.to_version != to_version {
            let status = UpdateStatus::new(
            &request,
            Operation::Finalize,
            request.operation_id.clone(),
            StatusCode::InvalidRequest,
            format!(
                "finalize targetVersion {:?} does not match the version staged for this nodeUpdateId ({:?})",
                to_version, staged.to_version
            ),
            from_version,
            to_version,
            started,
            Some(Utc::now()),
        );
            self.record_and_publish(status).await?;
            return Ok(LoopControl::Continue);
        }

        let in_progress = UpdateStatus::new(
            &request,
            Operation::Finalize,
            request.operation_id.clone(),
            StatusCode::InProgress,
            "finalizing update",
            from_version.clone(),
            to_version.clone(),
            started,
            None,
        );
        self.publish_status(&in_progress).await?;
        let current_ver = parse_nebraska_version(&from_version, "finalize");
        let mut client = TridentClient::connect(&self.config.trident.socket).await?;
        let result = self
            .run_with_status_heartbeat(
                in_progress,
                client.update_finalize(self.config.orchestration.finalize_timeout),
            )
            .await;
        if let Some(ref v) = current_ver {
            self.report_nebraska_event(&request, finalize_nebraska_report(v, &result))
                .await;
        }
        match result {
            Ok(_) => {
                let boot_marker = current_boot_marker()?;
                self.state.set_pending_commit(PendingCommit {
                    request: request.clone(),
                    operation_id: request.operation_id.clone(),
                    operation: Operation::Finalize,
                    from_version: from_version.clone(),
                    to_version: to_version.clone(),
                    started_utc: started,
                    boot_marker,
                })?;
                let terminal = finalize_success_status(
                    &request,
                    from_version.clone(),
                    to_version.clone(),
                    started,
                );
                if let Err(err) = self.state.remember_completed(terminal.clone()) {
                    log::warn!("failed to record finalize completion in state.json: {err}");
                }
                self.best_effort_publish_terminal(&terminal).await;
                match self.rebooter.reboot() {
                    Ok(()) => Ok(LoopControl::ExitForReboot),
                    Err(err) => {
                        self.state.clear_pending_commit()?;
                        if let Some(ref v) = current_ver {
                            self.report_nebraska_event(
                                &request,
                                NebraskaReport::Failed {
                                    previous: v.clone(),
                                    current: v.clone(),
                                },
                            )
                            .await;
                        }
                        let status = UpdateStatus::new(
                            &request,
                            Operation::Finalize,
                            request.operation_id.clone(),
                            StatusCode::AgentInternalError,
                            format!("finalize succeeded but reboot failed: {err}"),
                            from_version,
                            to_version,
                            started,
                            Some(Utc::now()),
                        );
                        self.record_and_publish(status).await?;
                        Ok(LoopControl::Continue)
                    }
                }
            }
            Err(err) => {
                self.state.clear_pending_commit()?;
                let status =
                    finalize_failure_status(&request, from_version, to_version, started, &err);
                self.record_and_publish(status).await?;
                Ok(LoopControl::Continue)
            }
        }
    }

    async fn handle_rollback(&self, request: UpdateRequest) -> Result<LoopControl, anyhow::Error> {
        let started = Utc::now();
        let from_version = Some(current_active_version());

        let mut client = TridentClient::connect(&self.config.trident.socket).await?;

        let staging = UpdateStatus::new(
            &request,
            Operation::Rollback,
            request.operation_id.clone(),
            StatusCode::InProgress,
            "staging rollback",
            from_version.clone(),
            None,
            started,
            None,
        );
        self.publish_status(&staging).await?;

        let stage_response = match self
            .run_with_status_heartbeat(
                staging,
                client.rollback_stage(self.config.orchestration.stage_timeout),
            )
            .await
        {
            Ok(response) => response,
            Err(err) => {
                let status = rollback_stage_failure_status(&request, from_version, started, &err);
                self.record_and_publish(status).await?;
                return Ok(LoopControl::Continue);
            }
        };

        if !matches!(
            stage_response.servicing_kind,
            Some(ServicingKind::ManualRollbackAb)
        ) {
            let status = UpdateStatus::new(
                &request,
                Operation::Rollback,
                request.operation_id.clone(),
                StatusCode::OperationFailed,
                "no AB rollback available to perform for this node",
                from_version,
                None,
                started,
                Some(Utc::now()),
            );
            self.record_and_publish(status).await?;
            return Ok(LoopControl::Continue);
        }

        let finalizing = UpdateStatus::new(
            &request,
            Operation::Rollback,
            request.operation_id.clone(),
            StatusCode::InProgress,
            "finalizing rollback",
            from_version.clone(),
            None,
            started,
            None,
        );
        self.publish_status(&finalizing).await?;
        match self
            .run_with_status_heartbeat(
                finalizing,
                client.rollback_finalize(self.config.orchestration.finalize_timeout),
            )
            .await
        {
            Ok(_) => {
                let boot_marker = current_boot_marker()?;
                self.state.set_pending_commit(PendingCommit {
                    request: request.clone(),
                    operation_id: request.operation_id.clone(),
                    operation: Operation::Rollback,
                    from_version: from_version.clone(),
                    to_version: None,
                    started_utc: started,
                    boot_marker,
                })?;
                let terminal =
                    rollback_finalize_success_status(&request, from_version.clone(), started);
                if let Err(err) = self.state.remember_completed(terminal.clone()) {
                    log::warn!("failed to record rollback completion in state.json: {err}");
                }
                self.best_effort_publish_terminal(&terminal).await;
                match self.rebooter.reboot() {
                    Ok(()) => Ok(LoopControl::ExitForReboot),
                    Err(err) => {
                        self.state.clear_pending_commit()?;
                        let status = UpdateStatus::new(
                            &request,
                            Operation::Rollback,
                            request.operation_id.clone(),
                            StatusCode::AgentInternalError,
                            format!("rollback finalize succeeded but reboot failed: {err}"),
                            from_version,
                            None,
                            started,
                            Some(Utc::now()),
                        );
                        self.record_and_publish(status).await?;
                        Ok(LoopControl::Continue)
                    }
                }
            }
            Err(err) => {
                self.state.clear_pending_commit()?;
                let status =
                    rollback_finalize_failure_status(&request, from_version, started, &err);
                self.record_and_publish(status).await?;
                Ok(LoopControl::Continue)
            }
        }
    }

    async fn resume_pending_commit(&self, pending: PendingCommit) -> Result<(), anyhow::Error> {
        let current_boot = current_boot_marker()?;
        if current_boot == pending.boot_marker {
            log::info!(
                "pending commit {} is still waiting for the reboot to happen",
                pending.operation_id
            );
            return Ok(());
        }

        let mut client = match TridentClient::connect(&self.config.trident.socket).await {
            Ok(client) => client,
            Err(err) => {
                let status = self
                    .reconstruct_without_state(
                        &pending.request,
                        pending.from_version.clone(),
                        Some(err.to_string()),
                    )
                    .await;
                self.state.clear_pending_commit()?;
                self.record_and_publish(status).await?;
                return Ok(());
            }
        };
        let in_progress = UpdateStatus::new(
            &pending.request,
            Operation::Commit,
            pending.operation_id.clone(),
            StatusCode::InProgress,
            "committing post-reboot state",
            pending.from_version.clone(),
            pending.to_version.clone(),
            pending.started_utc,
            None,
        );
        self.publish_status(&in_progress).await?;
        let result = client
            .commit(self.config.orchestration.finalize_timeout)
            .await;
        if pending.operation == Operation::Finalize {
            if let (Some(previous), Some(current)) = (
                parse_nebraska_version(&pending.from_version, "post-reboot commit"),
                parse_nebraska_version(&pending.to_version, "post-reboot commit"),
            ) {
                self.report_nebraska_event(
                    &pending.request,
                    commit_nebraska_report(&previous, &current, &result),
                )
                .await;
            }
        }
        let status = self.map_commit_result(&pending, result);
        self.state.clear_pending_commit()?;
        self.record_and_publish(status).await
    }

    fn map_commit_result(
        &self,
        pending: &PendingCommit,
        result: Result<CompletedResponse, TridentClientError>,
    ) -> UpdateStatus {
        commit_result_to_status(pending, result)
    }

    async fn reconstruct_without_state(
        &self,
        request: &UpdateRequest,
        from_version: Option<String>,
        connect_error: Option<String>,
    ) -> UpdateStatus {
        // state.json did not survive the reboot (or was never written, e.g.
        // the agent crashed before persisting pendingCommit). Per
        // accepted-design-v2.md §2.3's degraded path, reconstruct the answer by
        // calling commit() unconditionally rather than guessing from labels
        // or the target version alone - tridentd's commit() is self-checking
        // and its own (ServicingKind/RebootStatus/Result) response already
        // distinguishes "swap happened, run commit" from "reboot hasn't
        // happened yet" from "target armed but firmware fell back" far more
        // reliably than a bare version-string comparison could.
        if let Some(status) =
            reconstruct_precheck_status(request, from_version.clone(), connect_error.as_deref())
        {
            return status;
        }

        let mut client = match TridentClient::connect(&self.config.trident.socket).await {
            Ok(client) => client,
            Err(err) => {
                return UpdateStatus::new(
                    request,
                    request.operation.into(),
                    request.operation_id.clone(),
                    StatusCode::AgentInternalError,
                    format!("state.json missing after reboot and tridentd unreachable: {err}"),
                    from_version,
                    request.target_version.clone(),
                    Utc::now(),
                    Some(Utc::now()),
                );
            }
        };

        let started = Utc::now();
        let result = client
            .commit(self.config.orchestration.finalize_timeout)
            .await;
        // Same rationale as resume_pending_commit: only Finalize is a
        // Nebraska-tracked update; this degraded path is only reached for
        // Finalize|Rollback (reconstruct_precheck_status above), so
        // Rollback is implicitly excluded here too.
        if matches!(request.operation, RequestedOperation::Finalize) {
            if let (Some(previous), Some(current)) = (
                parse_nebraska_version(&from_version, "reconstructed post-reboot commit"),
                parse_nebraska_version(&request.target_version, "reconstructed post-reboot commit"),
            ) {
                self.report_nebraska_event(
                    request,
                    commit_nebraska_report(&previous, &current, &result),
                )
                .await;
            }
        }
        reconstruct_commit_result_to_status(request, from_version, started, result)
    }

    async fn record_and_publish(&self, status: UpdateStatus) -> Result<(), anyhow::Error> {
        let status = status.refreshed_for_write();
        self.state.remember_completed(status.clone())?;
        self.best_effort_publish_terminal(&status).await;
        Ok(())
    }

    async fn publish_status(&self, status: &UpdateStatus) -> Result<(), anyhow::Error> {
        let status = status.refreshed_for_write();
        let mut annotations = BTreeMap::new();
        let annotation_key = match status.operation {
            Operation::Commit => UPDATE_COMMIT_STATUS_ANNOTATION,
            _ => UPDATE_STATUS_ANNOTATION,
        };
        annotations.insert(
            annotation_key.to_string(),
            Some(serde_json::to_string(&status)?),
        );
        log::info!(
            "sending {annotation_key} annotation to node {}: {status:?}",
            self.config.kubernetes.node_name
        );
        self.k8s
            .patch_node_metadata(
                &self.config.kubernetes.node_name,
                BTreeMap::new(),
                annotations,
            )
            .await?;
        Ok(())
    }

    async fn best_effort_publish_terminal(&self, status: &UpdateStatus) {
        for _ in 0..FINAL_STATUS_PATCH_RETRIES {
            match self.publish_status(status).await {
                Ok(()) => return,
                Err(err) if self.is_node_gone_error(&err) => {
                    log::info!(
                        "stopping terminal status publish because node {} no longer exists",
                        self.config.kubernetes.node_name
                    );
                    return;
                }
                Err(_) => tokio::time::sleep(FINAL_STATUS_PATCH_BACKOFF).await,
            }
        }
    }

    fn is_node_gone_error(&self, err: &anyhow::Error) -> bool {
        matches!(
            err.downcast_ref::<K8sClientError>(),
            Some(K8sClientError::NodeGone)
        )
    }

    fn log_and_swallow_node_gone(&self, err: &anyhow::Error, context: &str) -> bool {
        if self.is_node_gone_error(err) {
            log::info!(
                "stopping trident-acl-agent while {}: node {} no longer exists",
                context,
                self.config.kubernetes.node_name
            );
            true
        } else {
            false
        }
    }

    async fn run_with_status_heartbeat<F>(&self, status: UpdateStatus, future: F) -> F::Output
    where
        F: Future,
    {
        tokio::pin!(future);
        let mut interval = tokio::time::interval(self.config.orchestration.heartbeat_interval);
        interval.tick().await;
        let mut stop_heartbeats = false;
        loop {
            tokio::select! {
                result = &mut future => return result,
                _ = interval.tick(), if !stop_heartbeats => {
                    if let Err(err) = self.publish_status(&status).await {
                        if self.is_node_gone_error(&err) {
                            log::info!(
                                "stopping heartbeats because node {} no longer exists",
                                self.config.kubernetes.node_name
                            );
                            stop_heartbeats = true;
                        } else {
                            log::warn!("failed to refresh in-progress status heartbeat: {err}");
                        }
                    }
                }
            }
        }
    }

    /// Sends a single Nebraska event report, logging (but never
    /// propagating) failure.
    ///
    /// This is deliberately best-effort: per the `nebraska` module's own
    /// docs, an instance that sends **no** events at all is always safe
    /// (Nebraska self-heals to Complete on the instance's next check at the
    /// new version), so a failed report here must never fail - or even
    /// delay past its own retries - the underlying Trident operation it
    /// describes. `complete_after_reboot` reports already retry internally
    /// (see `nebraska::Client::complete_after_reboot`); everything else is a
    /// single attempt, on the theory that the next stage/finalize/commit
    /// step (or self-heal) will re-establish correct Nebraska state anyway.
    ///
    /// Runs on a dedicated blocking thread: the nebraska client's
    /// `reqwest::blocking`-based transport cannot safely be dropped from
    /// inside an already-running async task (see the same rationale on
    /// `handle_stage`'s `check_for_update` call).
    async fn report_nebraska_event(&self, request: &UpdateRequest, report: NebraskaReport) {
        let Some(endpoint) = self.resolve_nebraska_endpoint(request) else {
            // Should not happen in practice: every call site only reaches
            // here after handle_stage has already required an endpoint for
            // this node update. Guard anyway since this is best-effort
            // telemetry, not something worth panicking over.
            log::warn!(
                "skipping Nebraska '{}' report: no Nebraska endpoint configured (no request.server override and no [nebraska].endpoint)",
                report.label()
            );
            return;
        };
        let app_id = self.resolve_nebraska_app_id(request);
        let track = self.resolve_nebraska_track(request);
        let machine_id = match crate::build_machine_id(NEBRASKA_MACHINE_ID_SOURCE) {
            Ok(id) => id,
            Err(err) => {
                log::warn!(
                    "skipping Nebraska '{}' report: failed to build machine id: {err}",
                    report.label()
                );
                return;
            }
        };
        let label = report.label();
        let result = tokio::task::spawn_blocking(move || {
            let client = NebraskaClient::new(endpoint, app_id, track, machine_id);
            match report {
                NebraskaReport::Progress { version, event } => {
                    client.report_progress(&version, event)
                }
                NebraskaReport::Completed { previous, current } => client
                    .complete_after_reboot(&previous, &current)
                    .map(|_| ()),
                NebraskaReport::Failed { previous, current } => {
                    client.report_failure(&previous, &current)
                }
            }
        })
        .await;
        match result {
            Ok(Ok(())) => log::debug!("reported Nebraska '{label}' event"),
            Ok(Err(err)) => log::warn!("Nebraska '{label}' event report failed: {err}"),
            Err(err) => log::warn!("Nebraska '{label}' event report task panicked: {err}"),
        }
    }
}

/// Parses `version` (e.g. an `UpdateStatus::from_version`/`to_version`
/// field) as a semver [`Version`] for use in a Nebraska event report,
/// logging and returning `None` rather than failing if it's absent or not
/// valid semver. Nebraska event reporting is best-effort telemetry (see
/// `Orchestrator::report_nebraska_event`), so a malformed/missing version
/// string must only skip the report, never the Trident operation it
/// describes.
fn current_boot_marker() -> Result<String, anyhow::Error> {
    let raw = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .context("failed to read /proc/sys/kernel/random/boot_id")?;
    let marker = raw.trim().to_string();
    if marker.is_empty() {
        anyhow::bail!("/proc/sys/kernel/random/boot_id was empty");
    }
    Ok(marker)
}

fn parse_nebraska_version(version: &Option<String>, context: &str) -> Option<Version> {
    let raw = version.as_deref()?;
    match Version::parse(raw) {
        Ok(v) => Some(v),
        Err(err) => {
            log::warn!(
                "skipping Nebraska event report for {context}: {raw:?} is not valid semver: {err}"
            );
            None
        }
    }
}

/// A request annotation that parsed as JSON but failed schema/semantic
/// validation (e.g. wrong schemaVersion, missing targetVersion for
/// stage/finalize, or a targetVersion present on a rollback request). Kept
/// distinct from "no request at all" so reconcile_node can surface an
/// InvalidRequest status instead of silently ignoring the annotation.
#[derive(Debug, Clone)]
struct InvalidRequest {
    node_update_id: Uuid,
    operation_id: String,
    operation: Operation,
    reason: String,
}

#[derive(Debug, Clone, Default)]
struct Snapshot {
    request: Option<UpdateRequest>,
    invalid_request: Option<InvalidRequest>,
    operation_status: Option<UpdateStatus>,
    commit_status: Option<UpdateStatus>,
}

impl Snapshot {
    fn from_node(node: &Node) -> Self {
        let annotations = node.metadata.annotations.as_ref();
        let raw_request = annotations.and_then(|a| a.get(UPDATE_REQUEST_ANNOTATION));
        let (request, invalid_request) = match raw_request
            .map(|v| serde_json::from_str::<UpdateRequest>(v))
        {
            None => (None, None),
            Some(Ok(candidate)) => match candidate.clone().validate() {
                Ok(valid) => (Some(valid), None),
                Err(reason) => (
                    None,
                    Some(InvalidRequest {
                        node_update_id: candidate.node_update_id,
                        operation_id: candidate.operation_id,
                        operation: candidate.operation.into(),
                        reason,
                    }),
                ),
            },
            Some(Err(err)) => {
                // Cannot attribute a status to an operationId we couldn't
                // even parse out of the annotation - log loudly instead so
                // this doesn't fail silently, but there's no request to
                // surface an InvalidRequest status against.
                log::warn!(
                    "ignoring malformed {UPDATE_REQUEST_ANNOTATION} annotation (JSON parse failed): {err}"
                );
                (None, None)
            }
        };
        let operation_status = annotations
            .and_then(|a| a.get(UPDATE_STATUS_ANNOTATION))
            .and_then(|v| serde_json::from_str::<UpdateStatus>(v).ok());
        let commit_status = annotations
            .and_then(|a| a.get(UPDATE_COMMIT_STATUS_ANNOTATION))
            .and_then(|v| serde_json::from_str::<UpdateStatus>(v).ok());
        Self {
            request,
            invalid_request,
            operation_status,
            commit_status,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoopControl {
    Continue,
    ExitForReboot,
}

/// Decides which Nebraska event a stage() result should produce, given the
/// instance's currently-running version (which - for stage - is the same
/// version on both success and failure; only the post-reboot commit ever
/// changes it). Pure function - see `stage_result_to_status` for rationale.
fn stage_nebraska_report(
    current_version: &Version,
    result: &Result<CompletedResponse, TridentClientError>,
) -> NebraskaReport {
    match result {
        Ok(_) => NebraskaReport::Progress {
            version: current_version.clone(),
            event: ProgressEvent::DownloadFinished,
        },
        Err(_) => NebraskaReport::Failed {
            previous: current_version.clone(),
            current: current_version.clone(),
        },
    }
}

/// Decides which Nebraska event a finalize() result should produce. Pure
/// function - see `stage_result_to_status` for rationale. (Used at the
/// finalize *RPC* call site only; the `Installed` progress event for a
/// success is sent directly at the call site since it doesn't depend on the
/// result shape.)
fn finalize_nebraska_report(
    current_version: &Version,
    result: &Result<CompletedResponse, TridentClientError>,
) -> NebraskaReport {
    match result {
        Ok(_) => NebraskaReport::Progress {
            version: current_version.clone(),
            event: ProgressEvent::Installed,
        },
        Err(_) => NebraskaReport::Failed {
            previous: current_version.clone(),
            current: current_version.clone(),
        },
    }
}

/// Decides which Nebraska event a post-reboot commit() result should
/// produce, discharging the commitment made by the progress events sent
/// during stage/finalize. Pure function - see `stage_result_to_status` for
/// rationale.
///
/// - A clean commit success reports `Completed`, moving the instance to the
///   new version.
/// - A commit asking for *another* reboot is not a state Nebraska has any
///   representation for; treat it as not-yet-complete and report `Failed`
///   (previous == current, since the instance is still effectively on the
///   old version from Nebraska's point of view) so a later check can grant
///   again rather than leaving the instance wedged in progress forever.
/// - A commit result indicating the update was reverted (health/reboot
///   check failure) or any other failure both report `Failed` for the same
///   reason: the instance did not end up on the new version.
fn commit_nebraska_report(
    previous_version: &Version,
    new_version: &Version,
    result: &Result<CompletedResponse, TridentClientError>,
) -> NebraskaReport {
    match result {
        Ok(response) if response.reboot_status == RebootStatus::RebootRequired => {
            NebraskaReport::Failed {
                previous: previous_version.clone(),
                current: previous_version.clone(),
            }
        }
        Ok(_) => NebraskaReport::Completed {
            previous: previous_version.clone(),
            current: new_version.clone(),
        },
        Err(_) => NebraskaReport::Failed {
            previous: previous_version.clone(),
            current: previous_version.clone(),
        },
    }
}

/// Maps a stage() result to the terminal `UpdateStatus` for that stage
/// attempt. Pure function: no I/O, no side effects - exists so tests can
/// exercise the full success/failure matrix against a fake tridentd without
/// needing a real Kubernetes API or state store.
fn stage_result_to_status(
    request: &UpdateRequest,
    from_version: Option<String>,
    to_version: Option<String>,
    started: chrono::DateTime<Utc>,
    result: Result<CompletedResponse, TridentClientError>,
) -> UpdateStatus {
    match result {
        Ok(_) => UpdateStatus::new(
            request,
            Operation::Stage,
            request.operation_id.clone(),
            StatusCode::Success,
            "stage completed",
            from_version,
            to_version,
            started,
            Some(Utc::now()),
        ),
        Err(err) => UpdateStatus::new(
            request,
            Operation::Stage,
            request.operation_id.clone(),
            StatusCode::OperationFailed,
            format!("stage failed: {err}"),
            from_version,
            to_version,
            started,
            Some(Utc::now()),
        ),
    }
}

/// Builds the terminal `UpdateStatus` for a successful finalize() call. Pure
/// function - see `stage_result_to_status` for rationale.
fn finalize_success_status(
    request: &UpdateRequest,
    from_version: Option<String>,
    to_version: Option<String>,
    started: chrono::DateTime<Utc>,
) -> UpdateStatus {
    UpdateStatus::new(
        request,
        Operation::Finalize,
        request.operation_id.clone(),
        StatusCode::Success,
        "finalize completed; rebooting for commit",
        from_version,
        to_version,
        started,
        Some(Utc::now()),
    )
}

/// Builds the terminal `UpdateStatus` for a failed finalize() call. Pure
/// function - see `stage_result_to_status` for rationale.
fn finalize_failure_status(
    request: &UpdateRequest,
    from_version: Option<String>,
    to_version: Option<String>,
    started: chrono::DateTime<Utc>,
    err: &TridentClientError,
) -> UpdateStatus {
    UpdateStatus::new(
        request,
        Operation::Finalize,
        request.operation_id.clone(),
        map_trident_failure(err),
        format!("finalize failed: {err}"),
        from_version,
        to_version,
        started,
        Some(Utc::now()),
    )
}

fn map_trident_failure(error: &TridentClientError) -> StatusCode {
    if indicates_target_boot_failed(error) {
        StatusCode::TargetBootFailed
    } else {
        StatusCode::OperationFailed
    }
}

fn indicates_target_boot_failed(error: &TridentClientError) -> bool {
    error
        .remote()
        .map(|remote| {
            // "ab-update-reboot-check"/"ab-update-health-check-commit-check"
            // are the forward-update (finalize/commit) reboot-check
            // subkinds (ServicingError::AbUpdateRebootCheck /
            // HealthChecksError::AbUpdateHealthCheckCommitCheck).
            // "manual-rollback-reboot-check" is the *rollback*-specific
            // sibling (ServicingError::ManualRollbackRebootCheck), emitted
            // when a post-rollback reboot's firmware A/B fallback lands on
            // the wrong slot. It is a distinct enum variant with its own
            // kebab-case serde subkind, not a copy of the forward-update
            // one - both must be checked here, or a real rollback
            // boot-fallback silently reports as generic OperationFailed
            // instead of TargetBootFailed.
            remote.subkind == "ab-update-reboot-check"
                || remote.subkind == "ab-update-health-check-commit-check"
                || remote.subkind == "manual-rollback-reboot-check"
        })
        .unwrap_or(false)
}

/// Pure function extracted from `Orchestrator::map_commit_result` so tests
/// can exercise it directly (with a mock-tridentd-driven `Result`) without
/// needing a full `Orchestrator` instance. See `stage_result_to_status` for
/// rationale.
/// Pre-flight checks for the state.json-missing degraded reconstruction
/// path (accepted-design-v2.md §2.3). Returns `Some(status)` when reconstruction
/// cannot proceed (tridentd already known-unreachable, or the outstanding
/// request isn't a finalize/rollback), or `None` when the caller should go
/// on to call tridentd's commit() to determine the real outcome.
fn reconstruct_precheck_status(
    request: &UpdateRequest,
    from_version: Option<String>,
    connect_error: Option<&str>,
) -> Option<UpdateStatus> {
    if let Some(err) = connect_error {
        return Some(UpdateStatus::new(
            request,
            request.operation.into(),
            request.operation_id.clone(),
            StatusCode::AgentInternalError,
            format!("state.json missing after reboot and tridentd unreachable: {err}"),
            from_version,
            request.target_version.clone(),
            Utc::now(),
            Some(Utc::now()),
        ));
    }

    if !matches!(
        request.operation,
        RequestedOperation::Finalize | RequestedOperation::Rollback
    ) {
        return Some(UpdateStatus::new(
            request,
            request.operation.into(),
            request.operation_id.clone(),
            StatusCode::AgentInternalError,
            "unable to reconstruct operation without state.json",
            from_version,
            request.target_version.clone(),
            Utc::now(),
            Some(Utc::now()),
        ));
    }

    None
}

/// Maps tridentd's commit() result to the terminal status for the
/// state.json-missing degraded reconstruction path (accepted-design-v2.md
/// §2.3). Always reports under the original operationId, mirroring the
/// normal post-reboot commit path in `commit_result_to_status`.
fn reconstruct_commit_result_to_status(
    request: &UpdateRequest,
    from_version: Option<String>,
    started: DateTime<Utc>,
    result: Result<CompletedResponse, TridentClientError>,
) -> UpdateStatus {
    match result {
        Ok(response) if response.reboot_status == RebootStatus::RebootRequired => UpdateStatus::new(
            request,
            Operation::Commit,
            request.operation_id.clone(),
            StatusCode::AgentInternalError,
            "state.json missing after reboot; commit requested another reboot",
            from_version,
            request.target_version.clone(),
            started,
            Some(Utc::now()),
        ),
        Ok(_) => UpdateStatus::new(
            request,
            Operation::Commit,
            request.operation_id.clone(),
            StatusCode::Success,
            "state.json missing after reboot; commit() confirmed the swap and completed",
            from_version,
            request.target_version.clone(),
            started,
            Some(Utc::now()),
        ),
        Err(err) if indicates_target_boot_failed(&err) => UpdateStatus::new(
            request,
            Operation::Commit,
            request.operation_id.clone(),
            StatusCode::TargetBootFailed,
            format!(
                "state.json missing after reboot; commit detected rollback to previous version: {err}"
            ),
            from_version,
            request.target_version.clone(),
            started,
            Some(Utc::now()),
        ),
        Err(err) => UpdateStatus::new(
            request,
            Operation::Commit,
            request.operation_id.clone(),
            map_trident_failure(&err),
            format!("state.json missing after reboot; commit failed: {err}"),
            from_version,
            request.target_version.clone(),
            started,
            Some(Utc::now()),
        ),
    }
}

fn commit_result_to_status(
    pending: &PendingCommit,
    result: Result<CompletedResponse, TridentClientError>,
) -> UpdateStatus {
    match result {
        Ok(response) if response.reboot_status == RebootStatus::RebootRequired => {
            UpdateStatus::new(
                &pending.request,
                Operation::Commit,
                pending.operation_id.clone(),
                StatusCode::AgentInternalError,
                "commit requested another reboot",
                pending.from_version.clone(),
                pending.to_version.clone(),
                pending.started_utc,
                Some(Utc::now()),
            )
        }
        Ok(_) => UpdateStatus::new(
            &pending.request,
            Operation::Commit,
            pending.operation_id.clone(),
            StatusCode::Success,
            "commit completed",
            pending.from_version.clone(),
            pending.to_version.clone(),
            pending.started_utc,
            Some(Utc::now()),
        ),
        Err(err) if indicates_target_boot_failed(&err) => UpdateStatus::new(
            &pending.request,
            Operation::Commit,
            pending.operation_id.clone(),
            StatusCode::TargetBootFailed,
            format!("commit detected rollback to previous version: {err}"),
            pending.from_version.clone(),
            pending.to_version.clone(),
            pending.started_utc,
            Some(Utc::now()),
        ),
        Err(err) => UpdateStatus::new(
            &pending.request,
            Operation::Commit,
            pending.operation_id.clone(),
            map_trident_failure(&err),
            format!("commit failed: {err}"),
            pending.from_version.clone(),
            pending.to_version.clone(),
            pending.started_utc,
            Some(Utc::now()),
        ),
    }
}

/// Builds the terminal `UpdateStatus` for a failed rollback_stage() call.
/// Pure function - see `stage_result_to_status` for rationale.
fn rollback_stage_failure_status(
    request: &UpdateRequest,
    from_version: Option<String>,
    started: chrono::DateTime<Utc>,
    err: &TridentClientError,
) -> UpdateStatus {
    UpdateStatus::new(
        request,
        Operation::Rollback,
        request.operation_id.clone(),
        map_trident_failure(err),
        format!("rollback stage failed: {err}"),
        from_version,
        None,
        started,
        Some(Utc::now()),
    )
}

/// Builds the terminal `UpdateStatus` for a successful rollback_finalize()
/// call. Pure function - see `stage_result_to_status` for rationale.
fn rollback_finalize_success_status(
    request: &UpdateRequest,
    from_version: Option<String>,
    started: chrono::DateTime<Utc>,
) -> UpdateStatus {
    UpdateStatus::new(
        request,
        Operation::Rollback,
        request.operation_id.clone(),
        StatusCode::Success,
        "rollback finalize completed; rebooting for commit",
        from_version,
        None,
        started,
        Some(Utc::now()),
    )
}

/// Builds the terminal `UpdateStatus` for a failed rollback_finalize() call.
/// Pure function - see `stage_result_to_status` for rationale.
fn rollback_finalize_failure_status(
    request: &UpdateRequest,
    from_version: Option<String>,
    started: chrono::DateTime<Utc>,
    err: &TridentClientError,
) -> UpdateStatus {
    UpdateStatus::new(
        request,
        Operation::Rollback,
        request.operation_id.clone(),
        map_trident_failure(err),
        format!("rollback finalize failed: {err}"),
        from_version,
        None,
        started,
        Some(Utc::now()),
    )
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use chrono::Utc;
    use uuid::Uuid;

    use super::*;
    use crate::{
        annotations::{RequestedOperation, SCHEMA_VERSION},
        mock_tridentd::{connect_mock_client, MockTridentdConfig, Outcome},
    };

    fn request(operation: RequestedOperation) -> UpdateRequest {
        UpdateRequest {
            schema_version: SCHEMA_VERSION.to_string(),
            node_update_id: Uuid::new_v4(),
            operation_id: "op-1".to_string(),
            operation,
            target_version: Some("2.0.0".to_string()),
            server: None,
            app_id: None,
            track: None,
        }
    }

    fn pending(operation: Operation) -> PendingCommit {
        PendingCommit {
            request: request(RequestedOperation::Finalize),
            operation_id: "op-1".to_string(),
            operation,
            from_version: Some("1.0.0".to_string()),
            to_version: Some("2.0.0".to_string()),
            started_utc: Utc::now(),
            boot_marker: "boot-1".to_string(),
        }
    }

    // --- rollback ---

    #[tokio::test]
    async fn rollback_stage_failure_maps_to_operation_failed() {
        let config = Arc::new(Mutex::new(MockTridentdConfig {
            rollback_stage: Some(Outcome::Failure {
                subkind: "some-rollback-stage-error",
                message: "disk full",
            }),
            ..Default::default()
        }));
        let mut client = connect_mock_client(config).await;
        let result = client
            .rollback_stage(std::time::Duration::from_secs(5))
            .await;

        let request = request(RequestedOperation::Rollback);
        let status = rollback_stage_failure_status(
            &request,
            Some("2.0.0".to_string()),
            Utc::now(),
            &result.unwrap_err(),
        );

        assert_eq!(status.code, StatusCode::OperationFailed);
        assert_eq!(status.operation, Operation::Rollback);
        assert!(status.message.contains("rollback stage failed"));
    }

    /// Regression coverage for the "rollback with nothing to roll back"
    /// fix: RollbackStage's response must carry the real ServicingKind
    /// (ManualRollbackAb for a real rollback, NoneRequired for a no-op) so
    /// handle_rollback() in this module can distinguish the two before
    /// finalizing/rebooting - see the `matches!(stage_response.servicing_kind, ..)`
    /// check there. This test pins the wire plumbing `TridentClient`
    /// depends on: a mocked RollbackStage response's servicing_kind must
    /// survive unchanged into `CompletedResponse`.
    #[tokio::test]
    async fn rollback_stage_success_reports_servicing_kind() {
        let config = Arc::new(Mutex::new(MockTridentdConfig {
            rollback_stage: Some(Outcome::Success {
                reboot_status: RebootStatus::RebootNotRequired,
                servicing_kind: Some(ServicingKind::ManualRollbackAb),
            }),
            ..Default::default()
        }));
        let mut client = connect_mock_client(config).await;
        let response = client
            .rollback_stage(std::time::Duration::from_secs(5))
            .await
            .expect("mocked rollback_stage should succeed");
        assert_eq!(
            response.servicing_kind,
            Some(ServicingKind::ManualRollbackAb)
        );
    }

    #[tokio::test]
    async fn rollback_stage_noop_reports_none_required_servicing_kind() {
        let config = Arc::new(Mutex::new(MockTridentdConfig {
            rollback_stage: Some(Outcome::Success {
                reboot_status: RebootStatus::RebootNotRequired,
                servicing_kind: Some(ServicingKind::NoneRequired),
            }),
            ..Default::default()
        }));
        let mut client = connect_mock_client(config).await;
        let response = client
            .rollback_stage(std::time::Duration::from_secs(5))
            .await
            .expect("mocked rollback_stage should succeed");
        assert_eq!(response.servicing_kind, Some(ServicingKind::NoneRequired));
    }

    #[tokio::test]
    async fn rollback_finalize_success_maps_to_success_status() {
        let config = Arc::new(Mutex::new(MockTridentdConfig {
            rollback_finalize: Some(Outcome::Success {
                reboot_status: RebootStatus::RebootRequired,
                servicing_kind: None,
            }),
            ..Default::default()
        }));
        let mut client = connect_mock_client(config).await;
        let result = client
            .rollback_finalize(std::time::Duration::from_secs(5))
            .await;
        assert!(result.is_ok());

        let request = request(RequestedOperation::Rollback);
        let status =
            rollback_finalize_success_status(&request, Some("2.0.0".to_string()), Utc::now());

        assert_eq!(status.code, StatusCode::Success);
        assert_eq!(status.operation, Operation::Rollback);
        assert_eq!(status.to_version, None);
    }

    #[tokio::test]
    async fn rollback_finalize_failure_maps_to_operation_failed() {
        let config = Arc::new(Mutex::new(MockTridentdConfig {
            rollback_finalize: Some(Outcome::Failure {
                subkind: "some-rollback-finalize-error",
                message: "boom",
            }),
            ..Default::default()
        }));
        let mut client = connect_mock_client(config).await;
        let result = client
            .rollback_finalize(std::time::Duration::from_secs(5))
            .await;

        let request = request(RequestedOperation::Rollback);
        let status = rollback_finalize_failure_status(
            &request,
            Some("2.0.0".to_string()),
            Utc::now(),
            &result.unwrap_err(),
        );

        assert_eq!(status.code, StatusCode::OperationFailed);
        assert_eq!(status.operation, Operation::Rollback);
        assert!(status.message.contains("rollback finalize failed"));
    }

    #[tokio::test]
    async fn rollback_finalize_reverted_maps_to_reverted_to_previous() {
        let config = Arc::new(Mutex::new(MockTridentdConfig {
            rollback_finalize: Some(Outcome::Failure {
                subkind: "ab-update-reboot-check",
                message: "reverted",
            }),
            ..Default::default()
        }));
        let mut client = connect_mock_client(config).await;
        let result = client
            .rollback_finalize(std::time::Duration::from_secs(5))
            .await;

        let request = request(RequestedOperation::Rollback);
        let status = rollback_finalize_failure_status(
            &request,
            Some("2.0.0".to_string()),
            Utc::now(),
            &result.unwrap_err(),
        );

        assert_eq!(status.code, StatusCode::TargetBootFailed);
    }

    // --- stage ---

    #[tokio::test]
    async fn stage_success_maps_to_success_status() {
        let config = Arc::new(Mutex::new(MockTridentdConfig {
            stage: Some(Outcome::Success {
                reboot_status: RebootStatus::RebootNotRequired,
                servicing_kind: None,
            }),
            ..Default::default()
        }));
        let mut client = connect_mock_client(config).await;
        let result = client
            .update_stage(
                &"http://example.test/image".parse().unwrap(),
                None,
                std::time::Duration::from_secs(5),
            )
            .await;

        let request = request(RequestedOperation::Stage);
        let status = stage_result_to_status(
            &request,
            Some("1.0.0".to_string()),
            Some("2.0.0".to_string()),
            Utc::now(),
            result,
        );

        assert_eq!(status.code, StatusCode::Success);
        assert_eq!(status.operation, Operation::Stage);
        assert_eq!(status.from_version, Some("1.0.0".to_string()));
        assert_eq!(status.to_version, Some("2.0.0".to_string()));
    }

    #[tokio::test]
    async fn stage_failure_maps_to_operation_failed() {
        let config = Arc::new(Mutex::new(MockTridentdConfig {
            stage: Some(Outcome::Failure {
                subkind: "some-stage-error",
                message: "disk full",
            }),
            ..Default::default()
        }));
        let mut client = connect_mock_client(config).await;
        let result = client
            .update_stage(
                &"http://example.test/image".parse().unwrap(),
                None,
                std::time::Duration::from_secs(5),
            )
            .await;

        let request = request(RequestedOperation::Stage);
        let status = stage_result_to_status(&request, None, None, Utc::now(), result);

        assert_eq!(status.code, StatusCode::OperationFailed);
        assert!(status.message.contains("stage failed"));
        assert!(status.message.contains("disk full"));
    }

    #[tokio::test]
    async fn stage_nebraska_report_success_is_download_finished() {
        let config = Arc::new(Mutex::new(MockTridentdConfig {
            stage: Some(Outcome::Success {
                reboot_status: RebootStatus::RebootNotRequired,
                servicing_kind: None,
            }),
            ..Default::default()
        }));
        let mut client = connect_mock_client(config).await;
        let result = client
            .update_stage(
                &"http://example.test/image".parse().unwrap(),
                None,
                std::time::Duration::from_secs(5),
            )
            .await;

        let version = semver::Version::new(1, 0, 0);
        let report = stage_nebraska_report(&version, &result);
        assert_eq!(
            report,
            NebraskaReport::Progress {
                version,
                event: ProgressEvent::DownloadFinished,
            }
        );
    }

    #[tokio::test]
    async fn stage_nebraska_report_failure_releases_wedge() {
        let config = Arc::new(Mutex::new(MockTridentdConfig {
            stage: Some(Outcome::Failure {
                subkind: "some-stage-error",
                message: "disk full",
            }),
            ..Default::default()
        }));
        let mut client = connect_mock_client(config).await;
        let result = client
            .update_stage(
                &"http://example.test/image".parse().unwrap(),
                None,
                std::time::Duration::from_secs(5),
            )
            .await;

        let version = semver::Version::new(1, 0, 0);
        let report = stage_nebraska_report(&version, &result);
        assert_eq!(
            report,
            NebraskaReport::Failed {
                previous: version.clone(),
                current: version,
            }
        );
    }

    // --- finalize ---

    #[tokio::test]
    async fn finalize_success_maps_to_success_status() {
        let status = finalize_success_status(
            &request(RequestedOperation::Finalize),
            Some("1.0.0".to_string()),
            Some("2.0.0".to_string()),
            Utc::now(),
        );

        assert_eq!(status.code, StatusCode::Success);
        assert_eq!(status.operation, Operation::Finalize);
        assert!(status.message.contains("rebooting"));
    }

    #[tokio::test]
    async fn finalize_failure_with_generic_error_maps_to_operation_failed() {
        let config = Arc::new(Mutex::new(MockTridentdConfig {
            finalize: Some(Outcome::Failure {
                subkind: "some-finalize-error",
                message: "partition swap failed",
            }),
            ..Default::default()
        }));
        let mut client = connect_mock_client(config).await;
        let err = client
            .update_finalize(std::time::Duration::from_secs(5))
            .await
            .unwrap_err();

        let status = finalize_failure_status(
            &request(RequestedOperation::Finalize),
            None,
            None,
            Utc::now(),
            &err,
        );

        assert_eq!(status.code, StatusCode::OperationFailed);
        assert!(status.message.contains("finalize failed"));
        assert!(status.message.contains("partition swap failed"));
    }

    #[tokio::test]
    async fn finalize_failure_with_reboot_check_subkind_maps_to_reverted() {
        let config = Arc::new(Mutex::new(MockTridentdConfig {
            finalize: Some(Outcome::Failure {
                subkind: "ab-update-reboot-check",
                message: "boot did not land on target partition",
            }),
            ..Default::default()
        }));
        let mut client = connect_mock_client(config).await;
        let err = client
            .update_finalize(std::time::Duration::from_secs(5))
            .await
            .unwrap_err();

        let status = finalize_failure_status(
            &request(RequestedOperation::Finalize),
            None,
            None,
            Utc::now(),
            &err,
        );

        assert_eq!(status.code, StatusCode::TargetBootFailed);
    }

    #[tokio::test]
    async fn finalize_nebraska_report_success_is_installed() {
        let config = Arc::new(Mutex::new(MockTridentdConfig {
            finalize: Some(Outcome::Success {
                reboot_status: RebootStatus::RebootRequired,
                servicing_kind: None,
            }),
            ..Default::default()
        }));
        let mut client = connect_mock_client(config).await;
        let result = client
            .update_finalize(std::time::Duration::from_secs(5))
            .await;

        let version = semver::Version::new(1, 0, 0);
        let report = finalize_nebraska_report(&version, &result);
        assert_eq!(
            report,
            NebraskaReport::Progress {
                version,
                event: ProgressEvent::Installed,
            }
        );
    }

    #[tokio::test]
    async fn finalize_nebraska_report_failure_releases_wedge() {
        let config = Arc::new(Mutex::new(MockTridentdConfig {
            finalize: Some(Outcome::Failure {
                subkind: "some-finalize-error",
                message: "partition swap failed",
            }),
            ..Default::default()
        }));
        let mut client = connect_mock_client(config).await;
        let result = client
            .update_finalize(std::time::Duration::from_secs(5))
            .await;

        let version = semver::Version::new(1, 0, 0);
        let report = finalize_nebraska_report(&version, &result);
        assert_eq!(
            report,
            NebraskaReport::Failed {
                previous: version.clone(),
                current: version,
            }
        );
    }

    // --- commit ---

    #[tokio::test]
    async fn commit_success_maps_to_success_status() {
        let config = Arc::new(Mutex::new(MockTridentdConfig {
            commit: Some(Outcome::Success {
                reboot_status: RebootStatus::RebootNotRequired,
                servicing_kind: None,
            }),
            ..Default::default()
        }));
        let mut client = connect_mock_client(config).await;
        let result = client.commit(std::time::Duration::from_secs(5)).await;

        let status = commit_result_to_status(&pending(Operation::Finalize), result);

        assert_eq!(status.code, StatusCode::Success);
        assert_eq!(status.operation, Operation::Commit);
    }

    #[tokio::test]
    async fn commit_success_but_reboot_required_maps_to_agent_internal_error() {
        let config = Arc::new(Mutex::new(MockTridentdConfig {
            commit: Some(Outcome::Success {
                reboot_status: RebootStatus::RebootRequired,
                servicing_kind: None,
            }),
            ..Default::default()
        }));
        let mut client = connect_mock_client(config).await;
        let result = client.commit(std::time::Duration::from_secs(5)).await;

        let status = commit_result_to_status(&pending(Operation::Finalize), result);

        assert_eq!(status.code, StatusCode::AgentInternalError);
        assert!(status.message.contains("another reboot"));
    }

    #[tokio::test]
    async fn commit_failure_with_generic_error_maps_to_operation_failed() {
        let config = Arc::new(Mutex::new(MockTridentdConfig {
            commit: Some(Outcome::Failure {
                subkind: "some-commit-error",
                message: "commit rpc failed",
            }),
            ..Default::default()
        }));
        let mut client = connect_mock_client(config).await;
        let result = client.commit(std::time::Duration::from_secs(5)).await;

        let status = commit_result_to_status(&pending(Operation::Finalize), result);

        assert_eq!(status.code, StatusCode::OperationFailed);
        assert!(status.message.contains("commit failed"));
    }

    #[tokio::test]
    async fn commit_failure_with_reboot_check_subkind_maps_to_reverted() {
        let config = Arc::new(Mutex::new(MockTridentdConfig {
            commit: Some(Outcome::Failure {
                subkind: "ab-update-reboot-check",
                message: "boot did not land on target partition",
            }),
            ..Default::default()
        }));
        let mut client = connect_mock_client(config).await;
        let result = client.commit(std::time::Duration::from_secs(5)).await;

        let status = commit_result_to_status(&pending(Operation::Finalize), result);

        assert_eq!(status.code, StatusCode::TargetBootFailed);
    }

    #[tokio::test]
    async fn commit_failure_with_health_check_subkind_maps_to_reverted() {
        let config = Arc::new(Mutex::new(MockTridentdConfig {
            commit: Some(Outcome::Failure {
                subkind: "ab-update-health-check-commit-check",
                message: "post-commit health check failed",
            }),
            ..Default::default()
        }));
        let mut client = connect_mock_client(config).await;
        let result = client.commit(std::time::Duration::from_secs(5)).await;

        let status = commit_result_to_status(&pending(Operation::Finalize), result);

        assert_eq!(status.code, StatusCode::TargetBootFailed);
    }

    #[tokio::test]
    async fn commit_nebraska_report_success_is_completed_with_new_version() {
        let config = Arc::new(Mutex::new(MockTridentdConfig {
            commit: Some(Outcome::Success {
                reboot_status: RebootStatus::RebootNotRequired,
                servicing_kind: None,
            }),
            ..Default::default()
        }));
        let mut client = connect_mock_client(config).await;
        let result = client.commit(std::time::Duration::from_secs(5)).await;

        let previous = semver::Version::new(1, 0, 0);
        let current = semver::Version::new(2, 0, 0);
        let report = commit_nebraska_report(&previous, &current, &result);
        assert_eq!(report, NebraskaReport::Completed { previous, current });
    }

    #[tokio::test]
    async fn commit_nebraska_report_reboot_required_reports_failed_not_completed() {
        // A commit asking for another reboot is not really "complete" from
        // Nebraska's point of view: the instance hasn't landed on the new
        // version, so this must not report Completed (which would tell
        // Nebraska the fleet's instance count moved when it hasn't).
        let config = Arc::new(Mutex::new(MockTridentdConfig {
            commit: Some(Outcome::Success {
                reboot_status: RebootStatus::RebootRequired,
                servicing_kind: None,
            }),
            ..Default::default()
        }));
        let mut client = connect_mock_client(config).await;
        let result = client.commit(std::time::Duration::from_secs(5)).await;

        let previous = semver::Version::new(1, 0, 0);
        let current = semver::Version::new(2, 0, 0);
        let report = commit_nebraska_report(&previous, &current, &result);
        assert_eq!(
            report,
            NebraskaReport::Failed {
                previous: previous.clone(),
                current: previous,
            }
        );
    }

    #[tokio::test]
    async fn commit_nebraska_report_reverted_reports_failed() {
        let config = Arc::new(Mutex::new(MockTridentdConfig {
            commit: Some(Outcome::Failure {
                subkind: "ab-update-reboot-check",
                message: "boot did not land on target partition",
            }),
            ..Default::default()
        }));
        let mut client = connect_mock_client(config).await;
        let result = client.commit(std::time::Duration::from_secs(5)).await;

        let previous = semver::Version::new(1, 0, 0);
        let current = semver::Version::new(2, 0, 0);
        let report = commit_nebraska_report(&previous, &current, &result);
        assert_eq!(
            report,
            NebraskaReport::Failed {
                previous: previous.clone(),
                current: previous,
            }
        );
    }

    // --- reconstruct_without_state (state.json missing after reboot) ---

    #[test]
    fn reconstruct_precheck_reports_agent_internal_error_when_tridentd_unreachable() {
        let request = request(RequestedOperation::Finalize);
        let status = reconstruct_precheck_status(
            &request,
            Some("1.0.0".to_string()),
            Some("connection refused"),
        )
        .expect("connect error should short-circuit reconstruction");

        assert_eq!(status.code, StatusCode::AgentInternalError);
        assert!(status.message.contains("tridentd unreachable"));
        assert_eq!(status.operation_id, request.operation_id);
    }

    #[test]
    fn reconstruct_precheck_reports_agent_internal_error_for_non_finalize_rollback_operation() {
        let request = request(RequestedOperation::Stage);
        let status = reconstruct_precheck_status(&request, None, None)
            .expect("stage requests cannot be reconstructed without state.json");

        assert_eq!(status.code, StatusCode::AgentInternalError);
        assert!(status
            .message
            .contains("unable to reconstruct operation without state.json"));
    }

    #[test]
    fn reconstruct_precheck_allows_finalize_and_rollback_through() {
        for operation in [RequestedOperation::Finalize, RequestedOperation::Rollback] {
            let request = request(operation);
            assert!(
                reconstruct_precheck_status(&request, None, None).is_none(),
                "expected {operation:?} to proceed to commit() reconstruction"
            );
        }
    }

    #[tokio::test]
    async fn reconstruct_commit_result_success_maps_to_success_status() {
        let config = Arc::new(Mutex::new(MockTridentdConfig {
            commit: Some(Outcome::Success {
                reboot_status: RebootStatus::RebootNotRequired,
                servicing_kind: None,
            }),
            ..Default::default()
        }));
        let mut client = connect_mock_client(config).await;
        let result = client.commit(std::time::Duration::from_secs(5)).await;

        let request = request(RequestedOperation::Finalize);
        let status = reconstruct_commit_result_to_status(
            &request,
            Some("1.0.0".to_string()),
            Utc::now(),
            result,
        );

        assert_eq!(status.code, StatusCode::Success);
        assert_eq!(status.operation, Operation::Commit);
        assert_eq!(status.operation_id, request.operation_id.clone());
        assert!(status.message.contains("commit() confirmed the swap"));
    }

    #[tokio::test]
    async fn reconstruct_commit_result_reboot_required_maps_to_agent_internal_error() {
        let config = Arc::new(Mutex::new(MockTridentdConfig {
            commit: Some(Outcome::Success {
                reboot_status: RebootStatus::RebootRequired,
                servicing_kind: None,
            }),
            ..Default::default()
        }));
        let mut client = connect_mock_client(config).await;
        let result = client.commit(std::time::Duration::from_secs(5)).await;

        let request = request(RequestedOperation::Finalize);
        let status = reconstruct_commit_result_to_status(&request, None, Utc::now(), result);

        assert_eq!(status.code, StatusCode::AgentInternalError);
        assert!(status.message.contains("requested another reboot"));
    }

    #[tokio::test]
    async fn reconstruct_commit_result_reverted_subkind_maps_to_reverted_to_previous() {
        let config = Arc::new(Mutex::new(MockTridentdConfig {
            commit: Some(Outcome::Failure {
                subkind: "ab-update-reboot-check",
                message: "boot did not land on target partition",
            }),
            ..Default::default()
        }));
        let mut client = connect_mock_client(config).await;
        let result = client.commit(std::time::Duration::from_secs(5)).await;

        let request = request(RequestedOperation::Rollback);
        let status = reconstruct_commit_result_to_status(&request, None, Utc::now(), result);

        assert_eq!(status.code, StatusCode::TargetBootFailed);
        assert!(status.message.contains("detected rollback"));
    }

    #[tokio::test]
    async fn reconstruct_commit_result_generic_failure_maps_to_operation_failed() {
        let config = Arc::new(Mutex::new(MockTridentdConfig {
            commit: Some(Outcome::Failure {
                subkind: "some-commit-error",
                message: "commit rpc failed",
            }),
            ..Default::default()
        }));
        let mut client = connect_mock_client(config).await;
        let result = client.commit(std::time::Duration::from_secs(5)).await;

        let request = request(RequestedOperation::Finalize);
        let status = reconstruct_commit_result_to_status(&request, None, Utc::now(), result);

        assert_eq!(status.code, StatusCode::OperationFailed);
        assert!(status.message.contains("commit failed"));
        // Regression check for DR-003: the generic-failure branch must use the
        // same Operation::Commit / shared operationId as every other
        // branch of this function, matching commit_result_to_status and the
        // doc comment above reconstruct_commit_result_to_status.
        assert_eq!(status.operation, Operation::Commit);
        assert_eq!(status.operation_id, request.operation_id.clone());
    }

    // --- parse_nebraska_version ---

    #[test]
    fn parse_nebraska_version_parses_valid_semver() {
        assert_eq!(
            parse_nebraska_version(&Some("1.2.3".to_string()), "test"),
            Some(semver::Version::new(1, 2, 3))
        );
    }

    #[test]
    fn parse_nebraska_version_returns_none_for_missing_version() {
        assert_eq!(parse_nebraska_version(&None, "test"), None);
    }

    #[test]
    fn parse_nebraska_version_returns_none_for_invalid_semver() {
        assert_eq!(
            parse_nebraska_version(&Some("not-a-version".to_string()), "test"),
            None
        );
    }
}
