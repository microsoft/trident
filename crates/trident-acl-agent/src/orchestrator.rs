//! The Trident ACL agent's reconcile loop: watches the Node's request
//! annotation, drives Trident (stage/finalize/rollback/commit) over gRPC,
//! and writes the status annotation back, including post-reboot.
//!
//! Implements the node-side control flow from `docs/update-trigger-design.md`:
//! https://msazure.visualstudio.com/One/_git/Compute-ACL-Update-Service?version=GC67946fff8f296e10217b70e063c896e6028ea843&path=/docs/update-trigger-design.md
//! (sections 2.1 "Trigger mechanism", 2.3 "Stage/finalize/rollback split
//! and post-reboot commit", and 2.5 "Rollback"). See that document for the
//! full state-machine rationale; keep it in sync with this file if the
//! design changes.

use std::{collections::BTreeMap, process::Command};

use anyhow::Context;
use chrono::{DateTime, Utc};
use futures::StreamExt;
use k8s_openapi::api::core::v1::Node;
use semver::Version;
use trident_proto::v1::{RebootStatus, ServicingKind};
use uuid::Uuid;

use crate::{
    annotations::{
        commit_operation_id, current_active_version, Operation, RequestedOperation, StatusCode,
        UpdateRequest, UpdateStatus, SCHEMA_VERSION, UPDATE_REQUEST_ANNOTATION,
        UPDATE_STATUS_ANNOTATION,
    },
    config::AgentConfig,
    k8s::NodeClient,
    query_for_update,
    state::{PendingCommit, StateStore},
    trident::{CompletedResponse, TridentClient, TridentClientError},
    IdSource, QueryResult, DEFAULT_NEBRASKA_TRACK,
};

const FINAL_STATUS_PATCH_RETRIES: usize = 3;
const FINAL_STATUS_PATCH_BACKOFF: std::time::Duration = std::time::Duration::from_secs(2);

#[derive(Clone, Default)]
pub struct SystemRebooter;

pub trait RebootHandle: Clone + Send + Sync + 'static {
    fn reboot(&self) -> Result<(), anyhow::Error>;
}

impl RebootHandle for SystemRebooter {
    fn reboot(&self) -> Result<(), anyhow::Error> {
        for candidate in [
            ("reboot", Vec::<&str>::new()),
            ("systemctl", vec!["reboot"]),
        ] {
            match Command::new(candidate.0)
                .args(candidate.1.iter().copied())
                .status()
            {
                Ok(status) if status.success() => return Ok(()),
                Ok(status) => log::warn!("{} exited with {}", candidate.0, status),
                Err(err) => log::warn!("failed to invoke {}: {err}", candidate.0),
            }
        }
        Err(anyhow::anyhow!(
            "failed to issue reboot via reboot or systemctl reboot"
        ))
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
        self.recover_from_trident_state().await?;
        let mut stream = self
            .k8s
            .watch_node(self.config.kubernetes.node_name.clone());
        while let Some(node) = stream.next().await {
            match self.reconcile_node(&node?).await? {
                LoopControl::Continue => {}
                LoopControl::ExitForReboot => return Ok(()),
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
            // See reconcile_node() for why we must also check the
            // commit-suffixed id: a finalize/rollback's post-reboot outcome
            // is recorded under `<operationId>.commit`, not the original
            // request's plain operationId.
            let cached = persisted
                .completed
                .get(&commit_operation_id(&request.operation_id))
                .or_else(|| persisted.completed.get(&request.operation_id))
                .cloned();
            if let Some(status) = cached {
                self.publish_status(&status).await?;
                return Ok(());
            }
        }

        // No pendingCommit survived (or none was ever written) and there's no
        // cached terminal status for the current request's operationId. If
        // the outstanding request is a finalize/rollback, this is exactly the
        // "state.json did not survive the reboot" degraded path from
        // accepted-design.md §2.3: the *status* annotation from before the
        // reboot (e.g. finalize's InProgress/Success) is still sitting in the
        // API server untouched - annotations live in etcd, not on the node -
        // so we cannot use "is there a status annotation at all" to detect
        // this case. We must always attempt reconstruction here rather than
        // falling through to the normal watch loop, which would otherwise
        // re-run handle_finalize from scratch against an empty local
        // `completed` map and incorrectly report NotStaged. Stage requests
        // don't reboot, so a crash there is safely retried by the normal
        // watch loop instead.
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
            "received node update: request={:?} status={:?}",
            snapshot.request,
            snapshot.status
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
                    finished_utc: Some(now),
                };
                self.record_and_publish(status).await?;
            }
            return Ok(LoopControl::Continue);
        }

        let Some(request) = snapshot.request.clone() else {
            return Ok(LoopControl::Continue);
        };

        // A finalize/rollback request yields two terminal statuses under two
        // operationIds on opposite sides of the reboot: the plain id (the
        // pre-reboot `finalize`/`rollback` terminal) and the `.commit`-suffixed
        // id (the post-reboot `commit` terminal written by
        // resume_pending_commit()/reconstruct_without_state() - see
        // accepted-design.md §2.3). Once the commit half has landed, the
        // *request* annotation is still the original finalize/rollback
        // request (annotations don't get cleared), so this reconcile can
        // fire again for the same request after the commit already
        // completed. Checking only the plain operationId here missed the
        // commit-suffixed entry and caused this reconcile to treat the
        // request as unfinished, re-publishing the stale pre-reboot
        // `finalize` status and clobbering the correct post-reboot `commit`
        // status the caller (AKS-RP) is actually waiting on. Prefer the
        // commit-suffixed entry when present since it reflects the more
        // recent, authoritative outcome.
        let cached = persisted
            .completed
            .get(&commit_operation_id(&request.operation_id))
            .or_else(|| persisted.completed.get(&request.operation_id))
            .cloned();
        if let Some(status) = cached {
            // Only (re-)publish if the status annotation isn't already
            // up to date. Publishing unconditionally here is dangerous:
            // publish_status() PATCHes the Node, which is itself an
            // update the watch stream observes, which re-triggers
            // reconcile_node() for the very same (already-completed)
            // request, causing an infinite self-sustaining PATCH loop
            // (observed as thousands of PATCH/watch cycles per second
            // with no forward progress). Comparing against the annotation
            // already on the node breaks that cycle while still repairing
            // a stale/missing annotation exactly once.
            if snapshot.status.as_ref() != Some(&status) {
                self.publish_status(&status).await?;
            }
            return Ok(LoopControl::Continue);
        }

        if let Some(pending) = persisted.pending_commit.as_ref() {
            // Reject on operationId, not nodeUpdateId: the actual conflict
            // this guard exists to prevent is "a second finalize/rollback
            // starts while one is still waiting for its post-reboot
            // commit" (accepted-design.md's in-flight conflict rule).
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

        self.publish_status(&UpdateStatus::new(
            &request,
            Operation::Stage,
            request.operation_id.clone(),
            StatusCode::InProgress,
            "staging update",
            from_version.clone(),
            to_version.clone(),
            started,
            None,
        ))
        .await?;
        let endpoint = self.config.nebraska.endpoint.clone().ok_or_else(|| {
            anyhow::anyhow!("annotation mode requires [nebraska].endpoint or CLI override")
        })?;
        // query_for_update() is a blocking call (reqwest::blocking under the
        // hood, see omaha::send) - calling it directly from this async fn
        // can panic ("Cannot drop a runtime in a context where blocking is
        // not allowed") because reqwest::blocking spins up its own inner
        // Tokio runtime per call, which isn't safe to tear down from inside
        // an already-running async task. Run it on a dedicated blocking
        // thread instead.
        let app_id = self.config.nebraska.app_id.clone();
        let endpoint_for_task = endpoint.clone();
        let response = tokio::task::spawn_blocking(move || {
            query_for_update(
                &endpoint_for_task,
                &app_id,
                DEFAULT_NEBRASKA_TRACK,
                &Version::new(0, 0, 0),
                IdSource::MachineIdHashed,
            )
        })
        .await
        .context("Nebraska query task panicked")??;
        let offered = match response.result {
            QueryResult::NoUpdate => {
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
            QueryResult::NewDocument(update) => update,
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

        let mut client = TridentClient::connect(&self.config.trident.socket).await?;
        let result = client
            .update_stage(
                &offered.url,
                offered.hash.as_deref(),
                self.config.orchestration.stage_timeout,
            )
            .await;
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
        let staged = completed.values().find(|s| {
            s.node_update_id == request.node_update_id
                && s.operation == Operation::Stage
                && matches!(s.code, StatusCode::Success | StatusCode::AlreadyAtTarget)
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
        // finalize must apply to whatever was actually staged: tridentd's
        // update_finalize() carries no target version of its own, it just
        // finalizes whatever is currently staged on disk. Without this
        // check, a finalize whose targetVersion differs from the recorded
        // stage's targetVersion would silently finalize the *staged*
        // version while the status annotation reported the *requested*
        // (different) toVersion - a silent version-skew in the status
        // channel that AKS-RP has no way to detect.
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

        self.publish_status(&UpdateStatus::new(
            &request,
            Operation::Finalize,
            request.operation_id.clone(),
            StatusCode::InProgress,
            "finalizing update",
            from_version.clone(),
            to_version.clone(),
            started,
            None,
        ))
        .await?;
        self.state.set_pending_commit(PendingCommit {
            request: request.clone(),
            operation_id: request.operation_id.clone(),
            operation: Operation::Finalize,
            from_version: from_version.clone(),
            to_version: to_version.clone(),
            started_utc: started,
        })?;
        let mut client = TridentClient::connect(&self.config.trident.socket).await?;
        match client
            .update_finalize(self.config.orchestration.finalize_timeout)
            .await
        {
            Ok(_) => {
                let terminal = finalize_success_status(
                    &request,
                    from_version.clone(),
                    to_version.clone(),
                    started,
                );
                // Record this terminal status under the *finalize* operationId
                // (not just the eventual "<id>.commit" one written after
                // commit()) before rebooting. Without this, if state.json
                // doesn't survive the reboot, the still-present finalize
                // request annotation gets reconciled again post-reboot with
                // an empty local `completed` map for "finalize-op" and
                // re-runs handle_finalize from scratch - incorrectly
                // reporting NotStaged even though finalize already
                // succeeded and a commit reconstruction already ran.
                if let Err(err) = self.state.remember_completed(terminal.clone()) {
                    log::warn!("failed to record finalize completion in state.json: {err}");
                }
                self.best_effort_publish_terminal(&terminal).await;
                match self.rebooter.reboot() {
                    Ok(()) => Ok(LoopControl::ExitForReboot),
                    Err(err) => {
                        self.state.clear_pending_commit()?;
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

        self.publish_status(&UpdateStatus::new(
            &request,
            Operation::Rollback,
            request.operation_id.clone(),
            StatusCode::InProgress,
            "staging rollback",
            from_version.clone(),
            None,
            started,
            None,
        ))
        .await?;

        let stage_response = match client
            .rollback_stage(self.config.orchestration.stage_timeout)
            .await
        {
            Ok(response) => response,
            Err(err) => {
                let status = rollback_stage_failure_status(&request, from_version, started, &err);
                self.record_and_publish(status).await?;
                return Ok(LoopControl::Continue);
            }
        };

        // tridentd's RollbackStage reports a no-op ("nothing to roll back")
        // the same way update/install do: Ok/Success with servicing_kind ==
        // NoneRequired, not an error - see servicing.proto's
        // ServicingResponse.servicing_kind and execute_rollback() in
        // engine/manual_rollback/mod.rs. Detect that here and stop before
        // finalize/reboot - otherwise a rollback request against a node
        // with an empty rollback chain (or in a non-rollbackable state)
        // would be reported as false Success to AKS-RP and would trigger
        // an unnecessary reboot for nothing. `None` is treated the same as
        // `NoneRequired` (fail closed) in case a future response omits the
        // field.
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

        self.publish_status(&UpdateStatus::new(
            &request,
            Operation::Rollback,
            request.operation_id.clone(),
            StatusCode::InProgress,
            "finalizing rollback",
            from_version.clone(),
            None,
            started,
            None,
        ))
        .await?;
        self.state.set_pending_commit(PendingCommit {
            request: request.clone(),
            operation_id: request.operation_id.clone(),
            operation: Operation::Rollback,
            from_version: from_version.clone(),
            to_version: None,
            started_utc: started,
        })?;
        match client
            .rollback_finalize(self.config.orchestration.finalize_timeout)
            .await
        {
            Ok(_) => {
                let terminal =
                    rollback_finalize_success_status(&request, from_version.clone(), started);
                // Same rationale as handle_finalize(): record the terminal
                // status under the rollback's plain operationId before
                // rebooting, so a lost state.json doesn't cause
                // recover_from_trident_state() to re-run handle_rollback
                // from scratch after finalize already succeeded.
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
        self.publish_status(&UpdateStatus::new(
            &pending.request,
            Operation::Commit,
            commit_operation_id(&pending.operation_id),
            StatusCode::InProgress,
            "committing post-reboot state",
            pending.from_version.clone(),
            pending.to_version.clone(),
            pending.started_utc,
            None,
        ))
        .await?;
        let result = client
            .commit(self.config.orchestration.finalize_timeout)
            .await;
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
        // accepted-design.md §2.3's degraded path, reconstruct the answer by
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
        reconstruct_commit_result_to_status(request, from_version, started, result)
    }

    async fn record_and_publish(&self, status: UpdateStatus) -> Result<(), anyhow::Error> {
        self.state.remember_completed(status.clone())?;
        // Once the status is recorded in state.json, publishing it to the
        // Node annotation is not allowed to be fatal: right after a reboot
        // the fake-apiserver/kubelet networking can still be settling
        // (transient "connection refused"), and letting that error
        // propagate would crash the whole process via `?` up through
        // run()/main(). Systemd then restarts the agent in a tight loop
        // (visible as repeated "Scheduled restart job" entries), and the
        // already-recorded status never makes it onto the Node - the
        // annotation just stays stale until something else nudges a
        // reconcile. Retry with backoff instead; the write is idempotent
        // since remember_completed() already happened.
        self.best_effort_publish_terminal(&status).await;
        Ok(())
    }

    async fn publish_status(&self, status: &UpdateStatus) -> Result<(), anyhow::Error> {
        let mut annotations = BTreeMap::new();
        annotations.insert(
            UPDATE_STATUS_ANNOTATION.to_string(),
            Some(serde_json::to_string(status)?),
        );
        log::info!(
            "sending {UPDATE_STATUS_ANNOTATION} annotation to node {}: {status:?}",
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
            if self.publish_status(status).await.is_ok() {
                return;
            }
            tokio::time::sleep(FINAL_STATUS_PATCH_BACKOFF).await;
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
    #[allow(dead_code)]
    status: Option<UpdateStatus>,
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
        let status = annotations
            .and_then(|a| a.get(UPDATE_STATUS_ANNOTATION))
            .and_then(|v| serde_json::from_str::<UpdateStatus>(v).ok());
        Self {
            request,
            invalid_request,
            status,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoopControl {
    Continue,
    ExitForReboot,
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
    if indicates_reverted(error) {
        StatusCode::RevertedToPrevious
    } else {
        StatusCode::OperationFailed
    }
}

fn indicates_reverted(error: &TridentClientError) -> bool {
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
            // instead of RevertedToPrevious.
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
/// path (accepted-design.md §2.3). Returns `Some(status)` when reconstruction
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
/// state.json-missing degraded reconstruction path (accepted-design.md
/// §2.3). Always reports under the `.commit`-suffixed operationId, mirroring
/// the normal post-reboot commit path in `commit_result_to_status`.
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
            commit_operation_id(&request.operation_id),
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
            commit_operation_id(&request.operation_id),
            StatusCode::Success,
            "state.json missing after reboot; commit() confirmed the swap and completed",
            from_version,
            request.target_version.clone(),
            started,
            Some(Utc::now()),
        ),
        Err(err) if indicates_reverted(&err) => UpdateStatus::new(
            request,
            Operation::Commit,
            commit_operation_id(&request.operation_id),
            StatusCode::RevertedToPrevious,
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
            commit_operation_id(&request.operation_id),
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
                commit_operation_id(&pending.operation_id),
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
            commit_operation_id(&pending.operation_id),
            StatusCode::Success,
            "commit completed",
            pending.from_version.clone(),
            pending.to_version.clone(),
            pending.started_utc,
            Some(Utc::now()),
        ),
        Err(err) if indicates_reverted(&err) => UpdateStatus::new(
            &pending.request,
            Operation::Commit,
            commit_operation_id(&pending.operation_id),
            StatusCode::RevertedToPrevious,
            format!("commit detected rollback to previous version: {err}"),
            pending.from_version.clone(),
            pending.to_version.clone(),
            pending.started_utc,
            Some(Utc::now()),
        ),
        Err(err) => UpdateStatus::new(
            &pending.request,
            Operation::Commit,
            commit_operation_id(&pending.operation_id),
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

        assert_eq!(status.code, StatusCode::RevertedToPrevious);
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

        assert_eq!(status.code, StatusCode::RevertedToPrevious);
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

        assert_eq!(status.code, StatusCode::RevertedToPrevious);
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

        assert_eq!(status.code, StatusCode::RevertedToPrevious);
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
        assert_eq!(
            status.operation_id,
            commit_operation_id(&request.operation_id)
        );
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

        assert_eq!(status.code, StatusCode::RevertedToPrevious);
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
        // same Operation::Commit / `.commit`-suffixed operationId as every other
        // branch of this function, matching commit_result_to_status and the
        // doc comment above reconstruct_commit_result_to_status.
        assert_eq!(status.operation, Operation::Commit);
        assert_eq!(
            status.operation_id,
            commit_operation_id(&request.operation_id)
        );
    }
}
