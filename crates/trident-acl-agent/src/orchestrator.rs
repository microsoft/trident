use std::{collections::BTreeMap, process::Command};

use anyhow::Context;
use chrono::Utc;
use futures::StreamExt;
use k8s_openapi::api::core::v1::Node;
use semver::Version;
use trident_proto::v1::RebootStatus;

use crate::{
    annotations::{
        commit_operation_id, current_active_version, Operation, RequestedOperation, StatusCode,
        UpdateRequest, UpdateStatus, UPDATE_REQUEST_ANNOTATION, UPDATE_STATUS_ANNOTATION,
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
            if request.node_update_id != pending.request.node_update_id {
                let started = Utc::now();
                let status = UpdateStatus::new(
                    &request,
                    request.operation.into(),
                    request.operation_id.clone(),
                    StatusCode::InvalidRequest,
                    "another finalize/rollback is waiting for post-reboot commit",
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
        let response = query_for_update(
            &endpoint,
            &self.config.nebraska.app_id,
            DEFAULT_NEBRASKA_TRACK,
            &Version::new(0, 0, 0),
            IdSource::MachineIdHashed,
        )?;
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
        let status = match client
            .update_stage(
                &offered.url,
                offered.hash.as_deref(),
                self.config.orchestration.stage_timeout,
            )
            .await
        {
            Ok(_) => UpdateStatus::new(
                &request,
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
                &request,
                Operation::Stage,
                request.operation_id.clone(),
                StatusCode::OperationFailed,
                format!("stage failed: {err}"),
                from_version,
                to_version,
                started,
                Some(Utc::now()),
            ),
        };
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
        let staged = completed.values().any(|s| {
            s.node_update_id == request.node_update_id
                && s.operation == Operation::Stage
                && matches!(s.code, StatusCode::Success | StatusCode::AlreadyAtTarget)
        });
        if !staged {
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
                let terminal = UpdateStatus::new(
                    &request,
                    Operation::Finalize,
                    request.operation_id.clone(),
                    StatusCode::Success,
                    "finalize completed; rebooting for commit",
                    from_version.clone(),
                    to_version.clone(),
                    started,
                    Some(Utc::now()),
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
                let status = UpdateStatus::new(
                    &request,
                    Operation::Finalize,
                    request.operation_id.clone(),
                    map_trident_failure(&err),
                    format!("finalize failed: {err}"),
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

    async fn handle_rollback(&self, request: UpdateRequest) -> Result<LoopControl, anyhow::Error> {
        let started = Utc::now();
        let from_version = Some(current_active_version());
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
        self.run_trident_cli(&["rollback", "--ab", "--allowed-operations=stage"])?;
        self.state.set_pending_commit(PendingCommit {
            request: request.clone(),
            operation_id: request.operation_id.clone(),
            operation: Operation::Rollback,
            from_version: from_version.clone(),
            to_version: None,
            started_utc: started,
        })?;
        self.best_effort_publish_terminal(&UpdateStatus::new(
            &request,
            Operation::Rollback,
            request.operation_id.clone(),
            StatusCode::Success,
            "rollback staged; finalizing rollback and rebooting",
            from_version.clone(),
            None,
            started,
            Some(Utc::now()),
        ))
        .await;
        self.run_trident_cli(&["rollback", "--allowed-operations=finalize"])?;
        Ok(LoopControl::ExitForReboot)
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
        if let Some(err) = connect_error {
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

        if !matches!(
            request.operation,
            RequestedOperation::Finalize | RequestedOperation::Rollback
        ) {
            return UpdateStatus::new(
                request,
                request.operation.into(),
                request.operation_id.clone(),
                StatusCode::AgentInternalError,
                "unable to reconstruct operation without state.json",
                from_version,
                request.target_version.clone(),
                Utc::now(),
                Some(Utc::now()),
            );
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
                request.operation.into(),
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

    fn run_trident_cli(&self, args: &[&str]) -> Result<(), anyhow::Error> {
        let status = Command::new("trident")
            .args(args)
            .status()
            .with_context(|| format!("failed to run trident {}", args.join(" ")))?;
        if status.success() {
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "trident {} exited with {}",
                args.join(" "),
                status
            ))
        }
    }
}

#[derive(Debug, Clone, Default)]
struct Snapshot {
    request: Option<UpdateRequest>,
    #[allow(dead_code)]
    status: Option<UpdateStatus>,
}

impl Snapshot {
    fn from_node(node: &Node) -> Self {
        let annotations = node.metadata.annotations.as_ref();
        let request = annotations
            .and_then(|a| a.get(UPDATE_REQUEST_ANNOTATION))
            .and_then(|v| serde_json::from_str::<UpdateRequest>(v).ok())
            .and_then(|r| r.validate().ok());
        let status = annotations
            .and_then(|a| a.get(UPDATE_STATUS_ANNOTATION))
            .and_then(|v| serde_json::from_str::<UpdateStatus>(v).ok());
        Self { request, status }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoopControl {
    Continue,
    ExitForReboot,
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
            remote.subkind == "ab-update-reboot-check"
                || remote.subkind == "ab-update-health-check-commit-check"
        })
        .unwrap_or(false)
}
