//! The Trident ACL agent's reconcile loop: watches the Node's request
//! annotation, drives Trident (stage/finalize/rollback/commit) over gRPC,
//! and writes the status annotation back, including post-reboot.
//!
//! Implements the node-side control flow (covering the trigger
//! mechanism, stage/finalize/rollback split with post-reboot commit, and
//! rollback). See the design doc for the full state-machine rationale;
//! keep it in sync with this file if the design changes.

use std::{collections::BTreeMap, future::Future, time::Duration};

use anyhow::{anyhow, Context, Error};
use chrono::{DateTime, Utc};
use futures::StreamExt;
use k8s_openapi::api::core::v1::Node;
use log::{debug, info, warn};
use semver::Version;
use tokio::{pin, select, task, time};
use url::Url;
use uuid::Uuid;

use osutils::{dependencies::Dependency, machine_id};
use trident_api::error::{HealthChecksError, ServicingError};
use trident_proto::v1::{RebootStatus, ServicingKind};

use crate::{
    annotations::{
        k8s::{K8sClientError, NodeClient},
        state::{PendingCommit, StateStore},
        AnnotationKeys, Operation, RequestedOperation, StatusCode, UpdateRequest, UpdateStatus,
        SCHEMA_VERSION,
    },
    core::{
        config::AgentConfig,
        nebraska::{CheckOutcome, Client as NebraskaClient, ProgressEvent},
        retry::{retry, RetryError},
        trident::{CompletedResponse, TridentClient, TridentClientError},
        version::{current_active_version, FALLBACK_ALWAYS_VERSION},
    },
    IdSource,
};

const FINAL_STATUS_PATCH_RETRIES: usize = 3;
const FINAL_STATUS_PATCH_BACKOFF: Duration = Duration::from_secs(2);

/// Given the previous `consecutive_errors` count and whether the new error
/// is a genuine connect-establishment failure (see
/// [`K8sClientError::is_connect_failure`]), returns the updated count.
/// Establishment failures accumulate toward `connect_max_tries`; anything
/// else resets to 0, since it's proof the API server was reachable. Pure so
/// it's trivially unit testable.
fn accumulate_watch_error(consecutive_errors: usize, is_connect_failure: bool) -> usize {
    if is_connect_failure {
        consecutive_errors + 1
    } else {
        0
    }
}

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

pub struct Orchestrator {
    config: AgentConfig,
    k8s: NodeClient,
    state: StateStore,
    annotation_keys: AnnotationKeys,
}

impl Orchestrator {
    pub async fn from_config(config: AgentConfig) -> Result<Self, Error> {
        let k8s = NodeClient::new(&config.kubernetes).await?;
        let annotation_keys = AnnotationKeys::new(&config.kubernetes.annotation_prefix);
        Ok(Self {
            state: StateStore::new(config.orchestration.state_path.clone()),
            config,
            k8s,
            annotation_keys,
        })
    }

    /// Issues a real `systemctl reboot`. Routed through the repo's
    /// centralized dependency runner so a missing systemctl binary or
    /// non-zero exit produces the same uniform, actionable error type used
    /// everywhere else in the codebase (see crates/trident/src/reboot.rs for
    /// the same pattern).
    fn reboot(&self) -> Result<(), Error> {
        Dependency::Systemctl
            .cmd()
            .arg("reboot")
            .run_and_check()
            .context("failed to issue systemctl reboot")
    }

    pub async fn run(&self) -> Result<(), Error> {
        match self.recover_from_trident_state().await {
            Ok(LoopControl::Continue) => {}
            Ok(LoopControl::ExitForReboot) => return Ok(()),
            Err(err) => {
                if self.log_and_swallow_node_gone(&err, "recovering persisted state") {
                    return Ok(());
                }
                return Err(err);
            }
        }
        let mut stream = self
            .k8s
            .watch_node(self.config.kubernetes.node_name.clone());
        // Consecutive *connect-establishment* failures tolerated per
        // `connect_max_tries` (`TRIDENT_ACL_AGENT_KUBERNETES_CONNECT_MAX_TRIES`),
        // so a transient watch reconnect hiccup doesn't abort the whole
        // orchestrator the way a bare `node?` would. No extra sleep here
        // (unlike `get_node_with_retry`): `kube::runtime::watcher`'s own
        // `default_backoff()` (see `k8s::NodeClient::watch_node`) already
        // delays before the stream's next reconnect attempt, so sleeping
        // `connect_backoff` here as well would only stack a second,
        // redundant delay on top of it - `connect_backoff` governs
        // `get_node_with_retry` only.
        //
        // Only errors that mean the watch could never be established at all
        // (see `K8sClientError::is_connect_failure`) count toward this
        // budget. An already-established watch that broke or was rejected
        // by the apiserver mid-stream is proof the server *is* reachable -
        // not the kind of failure `connect_max_tries` is meant to bound -
        // so it resets the count instead of accumulating. This also avoids
        // guessing recovery from wall-clock gaps between errors: a single
        // connect attempt fails fast, but a stalled, already-connected watch
        // can take much longer to surface as an error (e.g. via a read
        // timeout), so elapsed time alone can't reliably tell the two apart.
        let mut consecutive_errors = 0usize;
        while let Some(item) = stream.next().await {
            let node = match item {
                Ok(node) => {
                    consecutive_errors = 0;
                    node
                }
                Err(err) => {
                    let is_connect_failure = err.is_connect_failure();
                    let err: Error = err.into();
                    if self.log_and_swallow_node_gone(&err, "watching node") {
                        return Ok(());
                    }
                    consecutive_errors =
                        accumulate_watch_error(consecutive_errors, is_connect_failure);
                    if self
                        .config
                        .kubernetes
                        .connect_max_tries
                        .is_exhausted(consecutive_errors)
                    {
                        return Err(err);
                    }
                    warn!(
                        "transient error watching node {} (attempt {consecutive_errors}): {err:#}",
                        self.config.kubernetes.node_name
                    );
                    continue;
                }
            };
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

    /// Startup recovery. Order matters: a pending post-reboot `commit()` is
    /// resumed *before* the first Kubernetes call, since `commit()` is a
    /// purely local `tridentd` gRPC call and is exactly the time-sensitive
    /// step a k8s outage must not block (a node left uncommitted risks a
    /// second reboot silently falling back to the old slot). Only
    /// *publishing* the resulting status annotation needs k8s, and that
    /// publish is already best-effort/retried (see
    /// `best_effort_publish_terminal`), so deferring the k8s read this far
    /// costs nothing when k8s is healthy and avoids a crash-loop when it
    /// isn't.
    async fn recover_from_trident_state(&self) -> Result<LoopControl, Error> {
        let persisted = self.state.load()?;

        if let Some(pending) = persisted.pending_commit.clone() {
            self.resume_pending_commit(pending).await?;
            return Ok(LoopControl::Continue);
        }

        let node = self
            .get_node_with_retry(&self.config.kubernetes.node_name)
            .await?;
        let snapshot = Snapshot::from_node(&node, &self.annotation_keys);

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
                    return Ok(LoopControl::Continue);
                }
            }
        }

        if let Some(request) = snapshot.request {
            return self
                .reconstruct_without_pending_record(&request, snapshot.operation_status.as_ref())
                .await;
        }
        Ok(LoopControl::Continue)
    }

    async fn reconcile_node(&self, node: &Node) -> Result<LoopControl, Error> {
        let snapshot = Snapshot::from_node(node, &self.annotation_keys);
        debug!(
            "received node update: request={:?} operation_status={:?} commit_status={:?}",
            snapshot.request, snapshot.operation_status, snapshot.commit_status
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
            // commit" (the in-flight conflict rule).
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
            // Same operationId as the outstanding pendingCommit: this is a
            // retry/re-issue of the operation already armed and waiting on
            // (or ready to resume) its post-reboot commit, not a new
            // finalize/rollback to drive from scratch. Falling through to
            // handle_finalize/handle_rollback below would re-run
            // UpdateFinalize/RollbackFinalize against a boot that may
            // already be armed or in flight, and on failure would
            // clear_pending_commit and discard a boot the firmware still
            // has queued. Resume it the same way startup recovery does
            // instead.
            self.resume_pending_commit(pending.clone()).await?;
            return Ok(LoopControl::Continue);
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

    /// Resolves which Nebraska endpoint to use for `request`: its own
    /// `server` field. Every Nebraska call this `nodeUpdateId` makes -
    /// stage's update check plus every progress/completion event report -
    /// must go through this resolver rather than reading
    /// `self.config.nebraska.endpoint` directly, since Nebraska's
    /// per-instance state is tied to one specific server: mixing endpoints
    /// across one update's lifecycle would split that state across two
    /// servers.
    ///
    /// `stage`/`finalize` requests must
    /// carry `server` and there is deliberately no static-config fallback
    /// here: a fallback would let a node update from a source AKS-RP did
    /// not choose. `UpdateRequest::validate()` already rejects a
    /// stage/finalize missing it with `InvalidRequest` before the
    /// orchestrator ever reaches this resolver, so `None` here should not
    /// happen in practice; callers still treat it as absent (rather than
    /// panicking) as defense in depth.
    fn resolve_nebraska_endpoint(&self, request: &UpdateRequest) -> Option<Url> {
        request.server.clone()
    }

    /// Resolves which Nebraska app id to use for `request`: its own `appId`
    /// field. Same stage/finalize-required, no-fallback rules as
    /// [`resolve_nebraska_endpoint`] - see its docs.
    fn resolve_nebraska_app_id(&self, request: &UpdateRequest) -> Option<String> {
        request.app_id.clone()
    }

    /// Resolves which Nebraska track to use for `request`: its own `track`
    /// field. Same stage/finalize-required, no-fallback rules as
    /// [`resolve_nebraska_endpoint`] - see its docs.
    fn resolve_nebraska_track(&self, request: &UpdateRequest) -> Option<String> {
        request.track.clone()
    }

    async fn handle_stage(&self, request: UpdateRequest) -> Result<(), Error> {
        let started = Utc::now();
        let from_version = Some(current_active_version()?);
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
        // UpdateRequest::validate() already requires server/appId/track for
        // stage/finalize before this handler is ever reached (see
        // resolve_nebraska_endpoint's docs), so treat a missing one here as
        // an agent-internal error rather than silently defaulting - there
        // is deliberately no static-config fallback for the annotation flow.
        let endpoint = self.resolve_nebraska_endpoint(&request).ok_or_else(|| {
            anyhow!(
                "stage request has no request.server despite passing validation (nodeUpdateId {})",
                request.node_update_id
            )
        })?;
        let app_id = self.resolve_nebraska_app_id(&request).ok_or_else(|| {
            anyhow!(
                "stage request has no request.appId despite passing validation (nodeUpdateId {})",
                request.node_update_id
            )
        })?;
        let track = self.resolve_nebraska_track(&request).ok_or_else(|| {
            anyhow!(
                "stage request has no request.track despite passing validation (nodeUpdateId {})",
                request.node_update_id
            )
        })?;
        let machine_id = crate::build_machine_id(NEBRASKA_MACHINE_ID_SOURCE)?;
        let current_version = parse_nebraska_version(&from_version, "stage current version")
            .unwrap_or_else(|| {
                Version::parse(FALLBACK_ALWAYS_VERSION)
                    .expect("invariant: FALLBACK_ALWAYS_VERSION is valid semver")
            });
        let outcome = task::spawn_blocking({
            let current_version = current_version.clone();
            move || {
                let client = NebraskaClient::new(endpoint, app_id, track, machine_id);
                client.check_for_update(&current_version)
            }
        })
        .await
        .context("Nebraska query task panicked")?
        .context("Nebraska query failed")?;
        let offered = match outcome {
            CheckOutcome::UpToDate => {
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
            CheckOutcome::UpdateInProgress => {
                // A prior stage attempt reported DownloadStarted to
                // Nebraska (below) but never followed up with a terminal
                // event - e.g. the agent was killed, or the node rebooted,
                // mid-download, before update_stage returned. Nebraska's
                // update_in_progress flag for this instance never clears
                // on its own: its documented self-heal only fires once the
                // instance checks in *at the new version*, which can't
                // happen because the update never actually installed. Left
                // alone, every later stage attempt would hit this same
                // branch forever with no way out. Send the compensating
                // Failed event (documented at nebraska::client as clearing
                // update_in_progress and re-arming the instance) before
                // reporting failure, so a subsequent stage (new
                // operationId) has a real chance to succeed instead of
                // being permanently wedged.
                self.report_nebraska_event(
                    &request,
                    NebraskaReport::Failed {
                        previous: current_version.clone(),
                        current: current_version.clone(),
                    },
                )
                .await;
                let status = UpdateStatus::new(
                    &request,
                    Operation::Stage,
                    request.operation_id.clone(),
                    StatusCode::OperationFailed,
                    "Nebraska reported an update already in progress for this instance; cleared the stuck in-progress state so a retried stage can succeed",
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

    async fn handle_finalize(&self, request: UpdateRequest) -> Result<LoopControl, Error> {
        let started = Utc::now();
        let from_version = Some(current_active_version()?);
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
            Ok(response) if response.reboot_status == RebootStatus::RebootRequired => {
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
                    warn!("failed to record finalize completion in state.json: {err}");
                }
                self.best_effort_publish_terminal(&terminal).await;
                match self.reboot() {
                    Ok(()) => Ok(LoopControl::ExitForReboot),
                    Err(err) => {
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
                        self.state
                            .remember_completed_and_clear_pending(status.clone())?;
                        self.best_effort_publish_terminal(&status).await;
                        Ok(LoopControl::Continue)
                    }
                }
            }
            Ok(_) => {
                // update_finalize returned success but did not report a
                // reboot as required - nothing was actually armed (e.g.
                // nothing staged to finalize; mirrors handle_rollback's
                // ManualRollbackAb check below). Treating any Ok(_) as
                // "boot armed" here previously meant only the agent-local
                // NotStaged cache guard above stood between a no-op
                // finalize and a real reboot + a false-positive Success -
                // Trident's own response is the authoritative signal now.
                let status = UpdateStatus::new(
                    &request,
                    Operation::Finalize,
                    request.operation_id.clone(),
                    StatusCode::NotStaged,
                    "finalize completed without arming a reboot (nothing to finalize)",
                    from_version,
                    to_version,
                    started,
                    Some(Utc::now()),
                );
                self.record_and_publish(status).await?;
                Ok(LoopControl::Continue)
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

    async fn handle_rollback(&self, request: UpdateRequest) -> Result<LoopControl, Error> {
        let started = Utc::now();
        let from_version = Some(current_active_version()?);

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
                    warn!("failed to record rollback completion in state.json: {err}");
                }
                self.best_effort_publish_terminal(&terminal).await;
                match self.reboot() {
                    Ok(()) => Ok(LoopControl::ExitForReboot),
                    Err(err) => {
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
                        self.state
                            .remember_completed_and_clear_pending(status.clone())?;
                        self.best_effort_publish_terminal(&status).await;
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

    async fn resume_pending_commit(&self, pending: PendingCommit) -> Result<(), Error> {
        let current_boot = current_boot_marker()?;
        if current_boot == pending.boot_marker {
            // No reboot has happened since finalize/rollback armed this
            // boot - the agent restarted (crash, watchdog, crash-loop)
            // without the reboot ever taking effect. Re-issue it instead of
            // just waiting: previously this branch only logged and
            // returned, so a reboot inhibited/delayed past the original
            // process exit left the node armed-but-never-rebooting
            // forever, showing `finalize: Success` with no commit until an
            // external watchdog eventually wiped it.
            info!(
                "pending commit {} is still waiting for the reboot to happen; re-issuing reboot",
                pending.operation_id
            );
            if let Err(err) = self.reboot() {
                let now = Utc::now();
                let status = UpdateStatus::new(
                    &pending.request,
                    pending.operation,
                    pending.operation_id.clone(),
                    StatusCode::AgentInternalError,
                    format!("armed update is waiting for reboot, but re-issuing it failed: {err}"),
                    pending.from_version.clone(),
                    pending.to_version.clone(),
                    pending.started_utc,
                    Some(now),
                );
                self.state
                    .remember_completed_and_clear_pending(status.clone())?;
                self.best_effort_publish_terminal(&status).await;
            }
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
                self.state
                    .remember_completed_and_clear_pending(status.clone())?;
                self.best_effort_publish_terminal(&status).await;
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
        let status = commit_result_to_status(&pending, result);
        self.state
            .remember_completed_and_clear_pending(status.clone())?;
        self.best_effort_publish_terminal(&status).await;
        Ok(())
    }

    /// Reconstructs the post-reboot outcome for `request` when `state.json`
    /// has no `pendingCommit` for it but a reboot is already known (by the
    /// caller) to have happened - either because `resume_pending_commit`'s
    /// boot-marker check already confirmed it (its `connect()` failure
    /// branch below), or because [`reconstruct_without_pending_record`]
    /// independently confirmed the swap via `current_active_version()`
    /// before ever calling this. In that situation it's safe to call
    /// `commit()` unconditionally: tridentd's own (ServicingKind/
    /// RebootStatus/Result) response distinguishes "already committed"
    /// from "target armed but firmware fell back" reliably. Do not call
    /// this when a reboot has *not* been confirmed - see
    /// [`reconstruct_without_pending_record`] for that (genuinely
    /// ambiguous) case, which must never call `commit()` speculatively.
    async fn reconstruct_without_state(
        &self,
        request: &UpdateRequest,
        from_version: Option<String>,
        connect_error: Option<String>,
    ) -> UpdateStatus {
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

    /// Reconstructs whether a reboot even happened for `request` when
    /// `state.json` carries no `pendingCommit` at all for it (missing
    /// entirely, or lost across the reboot) - see design doc 2.3's
    /// degraded-recovery path. This is the genuinely ambiguous case:
    /// unlike `resume_pending_commit`'s boot-marker check, there is no
    /// local record proving a reboot occurred, so `commit()` - which is
    /// state-changing and can discard an armed-but-unbooted update, or
    /// silently report `Success` for a no-op - must never be called
    /// speculatively here.
    ///
    /// If `operation_status` is entirely absent, *or* it exists but belongs
    /// to a different `operationId` than `request` (e.g. a still-lingering
    /// terminal status annotation from an earlier, unrelated operation that
    /// hasn't been overwritten yet), the agent has no evidence this
    /// specific request was ever touched (its own status is set to
    /// `InProgress` immediately on dispatch, before anything else) - so the
    /// request is always run fresh rather than considered for `commit()`.
    /// A version match alone isn't enough proof here either: the node
    /// could already be on `targetVersion` for an unrelated reason (a
    /// prior, already-completed operation; a manually re-imaged node), in
    /// which case `handle_finalize`/`handle_rollback`'s own
    /// `AlreadyAtTarget` check produces the correct status without ever
    /// needing `commit()`.
    ///
    /// With an `operation_status` in hand, a reboot is considered confirmed
    /// (safe to call `commit()` via [`reconstruct_without_state`]) when
    /// either:
    /// - **active version == target version**: the swap already happened
    ///   (the active version only changes via the post-finalize swap), or
    /// - **the system's current boot started after `operation_status`'s
    ///   `finished_utc`** (see [`reboot_confirmed_since_arming`]):
    ///   `operation_status` is the finalize/rollback's own terminal status,
    ///   already published to the Node's `update-status` annotation
    ///   *before* the reboot was triggered (see the caller-handled-reboot
    ///   ordering in module docs), so it survives independent of local
    ///   disk state. If the node's current boot demonstrably started after
    ///   that timestamp, a reboot has happened since, whatever version the
    ///   node ended up on - so it's safe to call `commit()` and let
    ///   Trident's own response (already handled by
    ///   `reconstruct_commit_result_to_status`'s `indicates_target_boot_failed`
    ///   check) distinguish a successful commit from a firmware fallback.
    ///
    /// If neither is true, the request is run fresh instead of guessed at
    /// further - always safe (never touches `commit()` on an unconfirmed
    /// boot), though it means an *unconfirmable* firmware fallback (e.g.
    /// `operation_status` has no `finished_utc`, i.e. it's stuck at
    /// `InProgress`) is retried as a fresh finalize/rollback rather than
    /// reported as `TargetBootFailed`. Closing that last corner fully would
    /// need Trident's own boot history (`get rollback-chain` / `get
    /// last-error`), which isn't exposed over gRPC today, only via the CLI.
    ///
    /// `rollback` requests carry no explicit `targetVersion` (the target is
    /// implicit: whatever the previous partition was), so the version
    /// comparison can never match for them - they rely entirely on the
    /// boot-time check above. If that's also inconclusive, a
    /// state.json-missing rollback recovery falls to "run fresh", which is
    /// still safe: `handle_rollback`'s own `stage_response.servicing_kind`
    /// check already reports `OperationFailed` if the rollback already
    /// happened and nothing is left to roll back to, rather than a false
    /// `Success`.
    async fn reconstruct_without_pending_record(
        &self,
        request: &UpdateRequest,
        operation_status: Option<&UpdateStatus>,
    ) -> Result<LoopControl, Error> {
        if !matches!(
            request.operation,
            RequestedOperation::Finalize | RequestedOperation::Rollback
        ) {
            return Ok(LoopControl::Continue);
        }

        let operation_status =
            operation_status.filter(|status| status.operation_id == request.operation_id);
        let Some(operation_status) = operation_status else {
            return self.run_request_fresh(request).await;
        };

        let now = Utc::now();
        let current_version = match current_active_version() {
            Ok(version) => version,
            Err(err) => {
                let status = UpdateStatus::new(
                    request,
                    request.operation.into(),
                    request.operation_id.clone(),
                    StatusCode::AgentInternalError,
                    format!(
                        "unable to determine current active version to reconstruct recovery state: {err}"
                    ),
                    None,
                    request.target_version.clone(),
                    now,
                    Some(now),
                );
                self.record_and_publish(status).await?;
                return Ok(LoopControl::Continue);
            }
        };

        let already_at_target = request.target_version.as_deref() == Some(current_version.as_str());
        if already_at_target || reboot_confirmed_since_arming(Some(operation_status)) {
            let status = self.reconstruct_without_state(request, None, None).await;
            self.record_and_publish(status).await?;
            return Ok(LoopControl::Continue);
        }

        self.run_request_fresh(request).await
    }

    async fn run_request_fresh(&self, request: &UpdateRequest) -> Result<LoopControl, Error> {
        match request.operation {
            RequestedOperation::Finalize => self.handle_finalize(request.clone()).await,
            RequestedOperation::Rollback => self.handle_rollback(request.clone()).await,
            RequestedOperation::Stage => Ok(LoopControl::Continue),
        }
    }

    async fn record_and_publish(&self, status: UpdateStatus) -> Result<(), Error> {
        let status = status.refreshed_for_write();
        self.state.remember_completed(status.clone())?;
        self.best_effort_publish_terminal(&status).await;
        Ok(())
    }

    async fn publish_status(&self, status: &UpdateStatus) -> Result<(), Error> {
        let status = status.refreshed_for_write();
        let mut annotations = BTreeMap::new();
        let annotation_key = match status.operation {
            Operation::Commit => &self.annotation_keys.commit_status,
            _ => &self.annotation_keys.status,
        };
        annotations.insert(
            annotation_key.to_string(),
            Some(serde_json::to_string(&status)?),
        );
        info!(
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
                    info!(
                        "stopping terminal status publish because node {} no longer exists",
                        self.config.kubernetes.node_name
                    );
                    return;
                }
                Err(_) => time::sleep(FINAL_STATUS_PATCH_BACKOFF).await,
            }
        }
    }

    /// Reads the agent's own Node object with a bounded (or, if
    /// configured, unbounded) retry, so a transient Kubernetes hiccup at
    /// startup recovery doesn't propagate the first error straight into a
    /// process exit / crash-loop the way a bare
    /// `self.k8s.get_node(...).await?` would. Attempt count and backoff are
    /// controlled by `connect_max_tries`/`connect_backoff`, set via
    /// `TRIDENT_ACL_AGENT_KUBERNETES_CONNECT_MAX_TRIES`/`_CONNECT_BACKOFF`
    /// (see `core::config`). Only used by `recover_from_trident_state`'s
    /// "no pending commit to resume" branch; the pending-commit resume
    /// itself never touches k8s at all (see that function's docs).
    /// `NodeGone` is returned immediately without retrying, since it's
    /// terminal - the node was deleted, and no amount of retrying changes
    /// that.
    async fn get_node_with_retry(&self, name: &str) -> Result<Node, K8sClientError> {
        retry(
            self.config.kubernetes.connect_max_tries,
            self.config.kubernetes.connect_backoff,
            || async {
                match self.k8s.get_node(name).await {
                    Ok(node) => Ok(node),
                    Err(err @ K8sClientError::NodeGone) => Err(RetryError::Permanent(err)),
                    Err(err) => Err(RetryError::Transient(err)),
                }
            },
        )
        .await
    }

    fn is_node_gone_error(&self, err: &Error) -> bool {
        matches!(
            err.downcast_ref::<K8sClientError>(),
            Some(K8sClientError::NodeGone)
        )
    }

    fn log_and_swallow_node_gone(&self, err: &Error, context: &str) -> bool {
        if self.is_node_gone_error(err) {
            info!(
                "stopping trident-acl-agent while {context}: node {} no longer exists",
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
        pin!(future);
        let mut interval = time::interval(self.config.orchestration.heartbeat_interval);
        interval.tick().await;
        let mut stop_heartbeats = false;
        loop {
            select! {
                result = &mut future => return result,
                _ = interval.tick(), if !stop_heartbeats => {
                    if let Err(err) = self.publish_status(&status).await {
                        if self.is_node_gone_error(&err) {
                            info!(
                                "stopping heartbeats because node {} no longer exists",
                                self.config.kubernetes.node_name
                            );
                            stop_heartbeats = true;
                        } else {
                            warn!("failed to refresh in-progress status heartbeat: {err}");
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
            // Should not happen in practice: UpdateRequest::validate()
            // already requires request.server for stage/finalize before
            // the orchestrator ever reaches here, and there is
            // deliberately no static-config fallback (see
            // resolve_nebraska_endpoint's docs). Guard anyway since this is
            // best-effort telemetry, not something worth panicking over.
            warn!(
                "skipping Nebraska '{}' report: request has no server (nodeUpdateId {})",
                report.label(),
                request.node_update_id
            );
            return;
        };
        let Some(app_id) = self.resolve_nebraska_app_id(request) else {
            warn!(
                "skipping Nebraska '{}' report: request has no appId (nodeUpdateId {})",
                report.label(),
                request.node_update_id
            );
            return;
        };
        let Some(track) = self.resolve_nebraska_track(request) else {
            warn!(
                "skipping Nebraska '{}' report: request has no track (nodeUpdateId {})",
                report.label(),
                request.node_update_id
            );
            return;
        };
        let machine_id = match crate::build_machine_id(NEBRASKA_MACHINE_ID_SOURCE) {
            Ok(id) => id,
            Err(err) => {
                warn!(
                    "skipping Nebraska '{}' report: failed to build machine id: {err}",
                    report.label()
                );
                return;
            }
        };
        let label = report.label();
        let result = task::spawn_blocking(move || {
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
            Ok(Ok(())) => debug!("reported Nebraska '{label}' event"),
            Ok(Err(err)) => warn!("Nebraska '{label}' event report failed: {err}"),
            Err(err) => warn!("Nebraska '{label}' event report task panicked: {err}"),
        }
    }
}

/// Uses the kernel boot ID as the reboot marker because it changes on every
/// successful reboot but remains stable for the lifetime of the current boot.
/// That makes it a simple, durable fence for deciding whether a pending
/// finalize/rollback has crossed the reboot boundary yet.
fn current_boot_marker() -> Result<String, Error> {
    machine_id::boot_id()
}

/// Returns whether the system has rebooted since `operation_status` (the
/// finalize/rollback's own previously-published terminal status) was
/// recorded as finished, by comparing the current boot's start time
/// (`machine_id::boot_time()`, read from `/proc/stat`'s `btime` - no local
/// state persisted across the reboot required) against that status's
/// `finished_utc`. If the current boot started after the reboot was armed,
/// a reboot has definitely happened since - whatever version the node
/// ended up on. Never speculatively assumes a reboot happened: returns
/// `false` if there's no status to compare against, its `finished_utc` is
/// absent, or the boot time can't be read.
fn reboot_confirmed_since_arming(operation_status: Option<&UpdateStatus>) -> bool {
    let Some(finished_utc) = operation_status.and_then(|status| status.finished_utc) else {
        return false;
    };
    let Ok(boot_time_secs) = machine_id::boot_time() else {
        return false;
    };
    let Some(boot_time) = DateTime::from_timestamp(boot_time_secs, 0) else {
        return false;
    };
    boot_time > finished_utc
}

/// Parses `version` (e.g. an `UpdateStatus::from_version`/`to_version`
/// field) as a semver [`Version`] for use in a Nebraska event report,
/// logging and returning `None` rather than failing if it's absent or not
/// valid semver. Nebraska event reporting is best-effort telemetry (see
/// `Orchestrator::report_nebraska_event`), so a malformed/missing version
/// string must only skip the report, never the Trident operation it
/// describes.
fn parse_nebraska_version(version: &Option<String>, context: &str) -> Option<Version> {
    let raw = version.as_deref()?;
    match Version::parse(raw) {
        Ok(v) => Some(v),
        Err(err) => {
            warn!(
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
    fn from_node(node: &Node, keys: &AnnotationKeys) -> Self {
        let annotations = node.metadata.annotations.as_ref();
        let raw_request = annotations.and_then(|a| a.get(&keys.request));
        let (request, invalid_request) =
            match raw_request.map(|v| serde_json::from_str::<UpdateRequest>(v)) {
                None => (None, None),
                Some(Ok(candidate)) => match candidate.clone().validate() {
                    Ok(valid) => (Some(valid), None),
                    Err(reason) => (
                        None,
                        Some(InvalidRequest {
                            node_update_id: candidate.node_update_id,
                            operation_id: candidate.operation_id,
                            operation: candidate.operation.into(),
                            reason: reason.to_string(),
                        }),
                    ),
                },
                Some(Err(err)) => {
                    // Cannot attribute a status to an operationId we couldn't
                    // even parse out of the annotation - log loudly instead so
                    // this doesn't fail silently, but there's no request to
                    // surface an InvalidRequest status against.
                    warn!(
                        "ignoring malformed {} annotation (JSON parse failed): {err}",
                        keys.request
                    );
                    (None, None)
                }
            };
        let operation_status = annotations
            .and_then(|a| a.get(&keys.status))
            .and_then(|v| serde_json::from_str::<UpdateStatus>(v).ok());
        let commit_status = annotations
            .and_then(|a| a.get(&keys.commit_status))
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
    started: DateTime<Utc>,
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
    started: DateTime<Utc>,
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
    started: DateTime<Utc>,
    err: &TridentClientError,
) -> UpdateStatus {
    // Pre-reboot status: TargetBootFailed is reserved for the post-reboot
    // commit status (see map_trident_commit_failure's docs), so this always
    // reports OperationFailed regardless of the error's subkind.
    UpdateStatus::new(
        request,
        Operation::Finalize,
        request.operation_id.clone(),
        StatusCode::OperationFailed,
        format!("finalize failed: {err}"),
        from_version,
        to_version,
        started,
        Some(Utc::now()),
    )
}

/// Maps a `commit()` (or `update_finalize()`/`rollback_finalize()` sharing
/// the same reboot-check error subkinds) failure to a status code, for the
/// **post-reboot `commit` status only**. Per the status-code contract,
/// `TargetBootFailed` means the firmware fell back to the previous slot
/// after a real boot attempt, and is reserved for the `commit` key -
/// callers writing a pre-reboot `finalize`/`rollback` status must use
/// [`StatusCode::OperationFailed`] directly instead of this function, even
/// though the underlying Trident error subkinds are shared plumbing.
fn map_trident_commit_failure(error: &TridentClientError) -> StatusCode {
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
            remote.subkind == ServicingError::AB_UPDATE_REBOOT_CHECK_SUBKIND
                || remote.subkind == HealthChecksError::AB_UPDATE_HEALTH_CHECK_COMMIT_CHECK_SUBKIND
                || remote.subkind == ServicingError::MANUAL_ROLLBACK_REBOOT_CHECK_SUBKIND
        })
        .unwrap_or(false)
}

/// Pre-flight checks for the state.json-missing degraded reconstruction
/// path. Returns `Some(status)` when reconstruction
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
/// state.json-missing degraded reconstruction path. Always reports
/// under the original operationId, mirroring the
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
        // servicing_kind == NoneRequired means commit() found nothing to
        // commit (e.g. the node was already on its target with no armed
        // update to promote) - reporting Success here would tell AKS-RP a
        // real update completed when nothing actually did.
        Ok(response) if response.servicing_kind == Some(ServicingKind::NoneRequired) => {
            UpdateStatus::new(
                request,
                Operation::Commit,
                request.operation_id.clone(),
                StatusCode::OperationFailed,
                "state.json missing after reboot; commit() reported nothing to commit",
                from_version,
                request.target_version.clone(),
                started,
                Some(Utc::now()),
            )
        }
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
            map_trident_commit_failure(&err),
            format!("state.json missing after reboot; commit failed: {err}"),
            from_version,
            request.target_version.clone(),
            started,
            Some(Utc::now()),
        ),
    }
}

/// Maps a post-reboot commit RPC outcome to the status annotation. Pure
/// function so tests can exercise it directly (with a mock-tridentd-driven
/// `Result`) without needing a full `Orchestrator` instance. See
/// `stage_result_to_status` for rationale.
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
        // See reconstruct_commit_result_to_status's comment: a
        // NoneRequired servicing_kind means nothing was actually
        // committed, which must not be reported as Success even though
        // this path is normally only reached with a confirmed pending
        // commit (defense in depth against a stale/corrupted state.json
        // entry naming a commit that Trident no longer has anything armed
        // for).
        Ok(response) if response.servicing_kind == Some(ServicingKind::NoneRequired) => {
            UpdateStatus::new(
                &pending.request,
                Operation::Commit,
                pending.operation_id.clone(),
                StatusCode::OperationFailed,
                "commit() reported nothing to commit",
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
            map_trident_commit_failure(&err),
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
    started: DateTime<Utc>,
    err: &TridentClientError,
) -> UpdateStatus {
    // Pre-reboot status: see finalize_failure_status's comment -
    // TargetBootFailed is reserved for the post-reboot commit status.
    UpdateStatus::new(
        request,
        Operation::Rollback,
        request.operation_id.clone(),
        StatusCode::OperationFailed,
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
    started: DateTime<Utc>,
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
    started: DateTime<Utc>,
    err: &TridentClientError,
) -> UpdateStatus {
    // Pre-reboot status: see finalize_failure_status's comment -
    // TargetBootFailed is reserved for the post-reboot commit status.
    UpdateStatus::new(
        request,
        Operation::Rollback,
        request.operation_id.clone(),
        StatusCode::OperationFailed,
        format!("rollback finalize failed: {err}"),
        from_version,
        None,
        started,
        Some(Utc::now()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::{Arc, Mutex};

    use chrono::Utc;
    use uuid::Uuid;

    const MOCK_RPC_TIMEOUT: Duration = Duration::from_secs(5);
    use crate::{
        annotations::{RequestedOperation, SCHEMA_VERSION},
        core::trident::mock::{connect_mock_client, MockTridentdConfig, Outcome},
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

    // --- watch-error accumulation ---

    #[test]
    fn accumulate_watch_error_increments_on_connect_failure() {
        assert_eq!(accumulate_watch_error(0, true), 1);
        assert_eq!(accumulate_watch_error(2, true), 3);
    }

    /// An already-established watch that breaks (or is rejected by the
    /// apiserver) mid-stream is proof the API server *is* reachable, so it
    /// must not be mistaken for a connect-establishment failure - it should
    /// reset the count rather than accumulate toward `connect_max_tries`.
    /// This matters even when such an error takes a long time to surface
    /// (e.g. a stalled read taking most of a multi-minute timeout): unlike
    /// a wall-clock heuristic, classifying by error kind gets this right
    /// regardless of how slow the failure was to appear.
    #[test]
    fn accumulate_watch_error_resets_on_established_watch_failure() {
        assert_eq!(accumulate_watch_error(5, false), 0);
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
        let result = client.rollback_stage(MOCK_RPC_TIMEOUT).await;

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
            .rollback_stage(MOCK_RPC_TIMEOUT)
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
            .rollback_stage(MOCK_RPC_TIMEOUT)
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
        let result = client.rollback_finalize(MOCK_RPC_TIMEOUT).await;
        result.expect("mocked rollback_finalize should succeed");

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
        let result = client.rollback_finalize(MOCK_RPC_TIMEOUT).await;

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
    async fn rollback_finalize_reboot_check_subkind_maps_to_operation_failed() {
        // Pre-reboot rollback-finalize failures always report
        // OperationFailed now, even with a boot-check subkind -
        // TargetBootFailed is reserved for the post-reboot commit status.
        let config = Arc::new(Mutex::new(MockTridentdConfig {
            rollback_finalize: Some(Outcome::Failure {
                subkind: "ab-update-reboot-check",
                message: "reverted",
            }),
            ..Default::default()
        }));
        let mut client = connect_mock_client(config).await;
        let result = client.rollback_finalize(MOCK_RPC_TIMEOUT).await;

        let request = request(RequestedOperation::Rollback);
        let status = rollback_finalize_failure_status(
            &request,
            Some("2.0.0".to_string()),
            Utc::now(),
            &result.unwrap_err(),
        );

        assert_eq!(status.code, StatusCode::OperationFailed);
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
                MOCK_RPC_TIMEOUT,
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
                MOCK_RPC_TIMEOUT,
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
                MOCK_RPC_TIMEOUT,
            )
            .await;

        let version = Version::new(1, 0, 0);
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
                MOCK_RPC_TIMEOUT,
            )
            .await;

        let version = Version::new(1, 0, 0);
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
        let err = client.update_finalize(MOCK_RPC_TIMEOUT).await.unwrap_err();

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
    async fn finalize_failure_with_reboot_check_subkind_maps_to_operation_failed() {
        // Pre-reboot finalize failures always report OperationFailed now,
        // even when the underlying Trident error carries a boot-check
        // subkind - TargetBootFailed is reserved for the post-reboot
        // commit status (see finalize_failure_status's doc comment).
        let config = Arc::new(Mutex::new(MockTridentdConfig {
            finalize: Some(Outcome::Failure {
                subkind: "ab-update-reboot-check",
                message: "boot did not land on target partition",
            }),
            ..Default::default()
        }));
        let mut client = connect_mock_client(config).await;
        let err = client.update_finalize(MOCK_RPC_TIMEOUT).await.unwrap_err();

        let status = finalize_failure_status(
            &request(RequestedOperation::Finalize),
            None,
            None,
            Utc::now(),
            &err,
        );

        assert_eq!(status.code, StatusCode::OperationFailed);
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
        let result = client.update_finalize(MOCK_RPC_TIMEOUT).await;

        let version = Version::new(1, 0, 0);
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
        let result = client.update_finalize(MOCK_RPC_TIMEOUT).await;

        let version = Version::new(1, 0, 0);
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
        let result = client.commit(MOCK_RPC_TIMEOUT).await;

        let status = commit_result_to_status(&pending(Operation::Finalize), result);

        assert_eq!(status.code, StatusCode::Success);
        assert_eq!(status.operation, Operation::Commit);
    }

    #[tokio::test]
    async fn commit_success_with_none_required_servicing_kind_does_not_report_success() {
        // A NoneRequired servicing_kind means commit() found nothing to
        // commit - reporting Success here would tell AKS-RP a real update
        // completed when nothing actually did.
        let config = Arc::new(Mutex::new(MockTridentdConfig {
            commit: Some(Outcome::Success {
                reboot_status: RebootStatus::RebootNotRequired,
                servicing_kind: Some(ServicingKind::NoneRequired),
            }),
            ..Default::default()
        }));
        let mut client = connect_mock_client(config).await;
        let result = client.commit(MOCK_RPC_TIMEOUT).await;

        let status = commit_result_to_status(&pending(Operation::Finalize), result);

        assert_ne!(status.code, StatusCode::Success);
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
        let result = client.commit(MOCK_RPC_TIMEOUT).await;

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
        let result = client.commit(MOCK_RPC_TIMEOUT).await;

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
        let result = client.commit(MOCK_RPC_TIMEOUT).await;

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
        let result = client.commit(MOCK_RPC_TIMEOUT).await;

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
        let result = client.commit(MOCK_RPC_TIMEOUT).await;

        let previous = Version::new(1, 0, 0);
        let current = Version::new(2, 0, 0);
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
        let result = client.commit(MOCK_RPC_TIMEOUT).await;

        let previous = Version::new(1, 0, 0);
        let current = Version::new(2, 0, 0);
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
        let result = client.commit(MOCK_RPC_TIMEOUT).await;

        let previous = Version::new(1, 0, 0);
        let current = Version::new(2, 0, 0);
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
        let result = client.commit(MOCK_RPC_TIMEOUT).await;

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
    async fn reconstruct_commit_result_none_required_servicing_kind_does_not_report_success() {
        let config = Arc::new(Mutex::new(MockTridentdConfig {
            commit: Some(Outcome::Success {
                reboot_status: RebootStatus::RebootNotRequired,
                servicing_kind: Some(ServicingKind::NoneRequired),
            }),
            ..Default::default()
        }));
        let mut client = connect_mock_client(config).await;
        let result = client.commit(MOCK_RPC_TIMEOUT).await;

        let request = request(RequestedOperation::Finalize);
        let status = reconstruct_commit_result_to_status(
            &request,
            Some("1.0.0".to_string()),
            Utc::now(),
            result,
        );

        assert_ne!(status.code, StatusCode::Success);
        assert_eq!(status.operation, Operation::Commit);
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
        let result = client.commit(MOCK_RPC_TIMEOUT).await;

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
        let result = client.commit(MOCK_RPC_TIMEOUT).await;

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
        let result = client.commit(MOCK_RPC_TIMEOUT).await;

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
            Some(Version::new(1, 2, 3))
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

    // --- reboot_confirmed_since_arming ---

    #[test]
    fn reboot_confirmed_since_arming_true_for_ancient_finished_utc() {
        // Any real boot time is long after 1970 - a finished_utc from the
        // epoch must always read as "a reboot happened since".
        let epoch = DateTime::from_timestamp(0, 0).unwrap();
        let status = UpdateStatus::new(
            &request(RequestedOperation::Finalize),
            Operation::Finalize,
            "op-1".to_string(),
            StatusCode::Success,
            "finalize completed",
            Some("1.0.0".to_string()),
            Some("2.0.0".to_string()),
            epoch,
            Some(epoch),
        );

        assert!(reboot_confirmed_since_arming(Some(&status)));
    }

    #[test]
    fn reboot_confirmed_since_arming_false_for_far_future_finished_utc() {
        let far_future = DateTime::from_timestamp(32_503_680_000, 0).unwrap(); // ~year 3000
        let status = UpdateStatus::new(
            &request(RequestedOperation::Finalize),
            Operation::Finalize,
            "op-1".to_string(),
            StatusCode::Success,
            "finalize completed",
            Some("1.0.0".to_string()),
            Some("2.0.0".to_string()),
            far_future,
            Some(far_future),
        );

        assert!(!reboot_confirmed_since_arming(Some(&status)));
    }

    #[test]
    fn reboot_confirmed_since_arming_false_when_no_status() {
        assert!(!reboot_confirmed_since_arming(None));
    }

    #[test]
    fn reboot_confirmed_since_arming_false_when_finished_utc_absent() {
        let status = UpdateStatus::new(
            &request(RequestedOperation::Finalize),
            Operation::Finalize,
            "op-1".to_string(),
            StatusCode::InProgress,
            "finalizing",
            Some("1.0.0".to_string()),
            Some("2.0.0".to_string()),
            Utc::now(),
            None,
        );

        assert!(!reboot_confirmed_since_arming(Some(&status)));
    }
}
