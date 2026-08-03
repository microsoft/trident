//! Label-driven AKS orchestrator for Harpoon.
//!
//! This mirrors the design doc's sequence (§4), label schema (§3), and state
//! machine (§5). Opinionated choices resolving still-open questions are called
//! out inline:
//! * duplicate request-ids are idempotent re-affirmations, not restarts;
//! * label mode is opt-in via config only;
//! * stage/finalize timeouts default to 20m/10m placeholders pending tester data;
//! * `no-update-available` is distinct from hard stage failure.

use std::{collections::BTreeMap, process::Command};

use chrono::Utc;
use futures::StreamExt;
use k8s_openapi::api::core::v1::Node;
use semver::Version;
use serde::{Deserialize, Serialize};
use trident_proto::v1preview::ServicingState as PreviewServicingState;

use crate::{
    config::AgentConfig,
    k8s::NodeClient,
    labels::{
        FailureReason, State, UpdateRequest, FAILURE_DETAIL_ANNOTATION, FAILURE_REASON_LABEL,
        NODE_IMAGE_VERSION_LABEL, OBSERVED_REQUEST_ID_LABEL, REQUEST_ID_LABEL, REQUEST_LABEL,
        STATE_LABEL, TARGET_VERSION_LABEL,
    },
    query_for_update,
    trident::{RemoteError, TridentClient, TridentClientError},
    IdSource, QueryResult, DEFAULT_NEBRASKA_TRACK,
};

const FINALIZED_PATCH_RETRIES: usize = 3;
const FINALIZED_PATCH_BACKOFF: std::time::Duration = std::time::Duration::from_secs(2);

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
}

impl Orchestrator<SystemRebooter> {
    pub async fn from_config(config: AgentConfig) -> Result<Self, anyhow::Error> {
        let k8s = NodeClient::new(&config.kubernetes).await?;
        Ok(Self {
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
        let snapshot = ProtocolSnapshot::from_node(&node);

        let mut client = match TridentClient::connect(&self.config.trident.socket).await {
            Ok(client) => client,
            Err(err) => {
                log::warn!("unable to connect to tridentd during startup recovery: {err}");
                self.ensure_ready_label(&snapshot).await?;
                return Ok(());
            }
        };

        let trident_state = match client.get_servicing_state().await {
            Ok(state) => state,
            Err(err) => {
                log::warn!(
                    "failed to query trident servicing state before trusting labels; continuing with label view only: {err}"
                );
                self.ensure_ready_label(&snapshot).await?;
                return Ok(());
            }
        };

        let recovered_request_id = snapshot
            .request_id
            .clone()
            .or(snapshot.observed_request_id.clone());
        let recovered_target_version = snapshot
            .target_version
            .clone()
            .or(snapshot.node_image_version.clone());

        match trident_state {
            PreviewServicingState::UpdateAbStaged => {
                self.patch_protocol(
                    &self.config.kubernetes.node_name,
                    ProtocolPatch::progress(
                        State::Staged,
                        recovered_request_id,
                        recovered_target_version,
                    ),
                )
                .await?;
            }
            PreviewServicingState::UpdateAbFinalized => {
                // A DIFFERENT process restart than a real reboot can also land
                // tridentd in `UpdateAbFinalized`: the agent finalized
                // successfully, then crashed/restarted before (or during) its
                // own `reboot()` call, so the machine never actually rebooted
                // and we're still running from the pre-update boot. Per
                // tridentd's contract, that case reports `UpdateAbFinalized`
                // with no last error, whereas a genuine post-reboot resume
                // reports either `Provisioned` (handled below) or
                // `UpdateAbFinalized` *with* a last error recorded from the
                // reboot/health-check path. Distinguish the two via
                // `GetLastError` before deciding whether to retry the reboot
                // or proceed to `commit()`.
                let last_error = match client.get_last_error().await {
                    Ok(last_error) => last_error,
                    Err(err) => {
                        log::warn!(
                            "failed to query trident last-error while distinguishing reboot from restart; treating as a completed reboot and proceeding to commit: {err}"
                        );
                        Some(RemoteError {
                            kind: None,
                            subkind: "agent-local-last-error-query-failed".to_string(),
                            message: err.to_string(),
                            error_message: String::new(),
                        })
                    }
                };

                if last_error.is_none() {
                    log::warn!(
                        "tridentd reports UpdateAbFinalized with no last error: the agent restarted without the machine actually rebooting; retrying reboot instead of committing"
                    );
                    match self.rebooter.reboot() {
                        Ok(()) => {
                            // The process is expected to be terminated by the
                            // reboot; if it isn't (e.g. running under a
                            // supervisor that doesn't kill on shutdown
                            // signal), fall through to Ready so the next
                            // startup re-evaluates rather than looping here.
                        }
                        Err(err) => {
                            self.fail_request(
                                recovered_request_id,
                                FailureReason::FinalizeFailed,
                                None,
                                &format!(
                                    "finalize previously succeeded but re-issuing reboot after an agent restart failed: {err}"
                                ),
                            )
                            .await?;
                        }
                    }
                } else {
                    self.handle_commit(
                        recovered_request_id,
                        recovered_target_version,
                        trident_state,
                    )
                    .await?;
                }
            }
            PreviewServicingState::UpdateAbHealthCheckFailed => {
                // This state is only reachable after the host has actually
                // booted into the target OS and health checks ran (see the
                // `ServicingState` proto docs), so - unlike
                // `UpdateAbFinalized` - it unambiguously implies a real
                // reboot already occurred; no last-error check is needed to
                // disambiguate it from a bare process restart.
                self.handle_commit(
                    recovered_request_id,
                    recovered_target_version,
                    trident_state,
                )
                .await?;
            }
            PreviewServicingState::Provisioned => {
                // A real reboot landed us back at `Provisioned` (with or
                // without a last error, e.g. after a completed commit or a
                // rollback). If our labels still show a transitional
                // in-flight state for this request-id, that operation is
                // stale/orphaned - resolve it now instead of leaving a
                // dangling `staging`/`finalizing` label the steady-state loop
                // would otherwise `Reaffirm` forever (see the transitional
                // recovery case below for the same failure-mode on a mere
                // process restart).
                match snapshot.state {
                    Some(State::Staging) | Some(State::Finalizing) | Some(State::Committing) => {
                        self.fail_request(
                            recovered_request_id,
                            FailureReason::Timeout,
                            None,
                            &format!(
                                "node rebooted/returned to Provisioned while the agent's last known state was {:?}; the in-flight operation was interrupted and must be retried with a new request-id",
                                snapshot.state
                            ),
                        )
                        .await?;
                    }
                    _ => {
                        self.ensure_ready_label(&snapshot).await?;
                    }
                }
            }
            _ => {
                // Covers e.g. NotProvisioned/InstallStaged/InstallFinalized -
                // not part of the A/B update cycle. If labels still show a
                // transitional `staging`/`finalizing`/`committing` state left
                // over from a crash mid-operation (the process died before
                // tridentd's state ever advanced past where it started),
                // there is no in-progress tridentd operation to resume or
                // observe: fail the request explicitly instead of silently
                // reaffirming the stale label forever. This turns the
                // previously-permanent silent stall (DR-001) into an
                // actionable, terminal `failed` state the RP can react to
                // (e.g. by retrying with a fresh request-id).
                match snapshot.state {
                    Some(State::Staging) | Some(State::Finalizing) | Some(State::Committing) => {
                        self.fail_request(
                            recovered_request_id,
                            FailureReason::Timeout,
                            None,
                            &format!(
                                "agent restarted with trident servicing state {trident_state:?}, which does not match the in-flight label state {:?}; the prior operation was interrupted (e.g. a crash) and must be retried with a new request-id",
                                snapshot.state
                            ),
                        )
                        .await?;
                    }
                    _ => {
                        self.ensure_ready_label(&snapshot).await?;
                    }
                }
            }
        }

        Ok(())
    }

    async fn reconcile_node(&self, node: &Node) -> Result<LoopControl, anyhow::Error> {
        let snapshot = ProtocolSnapshot::from_node(node);
        match decide_action(&snapshot) {
            RequestedAction::None => {
                self.ensure_ready_label(&snapshot).await?;
                Ok(LoopControl::Continue)
            }
            RequestedAction::Reaffirm(state) => {
                self.reaffirm_snapshot(&snapshot, state).await?;
                Ok(LoopControl::Continue)
            }
            RequestedAction::Stage {
                request_id,
                target_version,
            } => {
                self.handle_stage(&snapshot, request_id, target_version)
                    .await
            }
            RequestedAction::Finalize { request_id } => {
                self.handle_finalize(&snapshot, request_id).await
            }
        }
    }

    async fn ensure_ready_label(&self, snapshot: &ProtocolSnapshot) -> Result<(), anyhow::Error> {
        if snapshot.request == UpdateRequest::None && snapshot.state.is_none() {
            self.patch_protocol(
                &self.config.kubernetes.node_name,
                ProtocolPatch::progress(State::Ready, None, snapshot.node_image_version.clone()),
            )
            .await?;
        }
        Ok(())
    }

    async fn reaffirm_snapshot(
        &self,
        snapshot: &ProtocolSnapshot,
        state: State,
    ) -> Result<(), anyhow::Error> {
        self.patch_protocol(
            &self.config.kubernetes.node_name,
            ProtocolPatch {
                state: Some(state),
                observed_request_id: snapshot
                    .observed_request_id
                    .clone()
                    .or(snapshot.request_id.clone()),
                failure_reason: snapshot.failure_reason,
                failure_detail: snapshot.failure_detail.clone(),
                node_image_version: snapshot.node_image_version.clone(),
                clear_failure: snapshot.failure_reason.is_none(),
            },
        )
        .await
    }

    async fn handle_stage(
        &self,
        snapshot: &ProtocolSnapshot,
        request_id: String,
        target_version: String,
    ) -> Result<LoopControl, anyhow::Error> {
        self.patch_protocol(
            &self.config.kubernetes.node_name,
            ProtocolPatch::progress(
                State::Staging,
                Some(request_id.clone()),
                snapshot.node_image_version.clone(),
            ),
        )
        .await?;

        let endpoint = self.config.nebraska.endpoint.clone().ok_or_else(|| {
            anyhow::anyhow!(
                "label mode requires [nebraska].endpoint or a CLI override; neither was provided"
            )
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
                self.fail_request(
                    Some(request_id),
                    FailureReason::NoUpdateAvailable,
                    None,
                    "Nebraska currently offers no update for the requested stage operation",
                )
                .await?;
                return Ok(LoopControl::Continue);
            }
            QueryResult::NewDocument(update) => update,
        };

        if let Err(reason) =
            evaluate_version_gate(&target_version, Some(&offered.version.to_string()))
        {
            self.fail_request(
                Some(request_id),
                reason,
                None,
                &format!(
                    "requested target version {target_version} but Nebraska currently offers {}",
                    offered.version
                ),
            )
            .await?;
            return Ok(LoopControl::Continue);
        }

        let mut client = TridentClient::connect(&self.config.trident.socket).await?;
        match client
            .update_stage(
                &offered.url,
                offered.hash.as_deref(),
                self.config.orchestration.stage_timeout,
            )
            .await
        {
            Ok(_) => {
                self.patch_protocol(
                    &self.config.kubernetes.node_name,
                    ProtocolPatch::progress(
                        State::Staged,
                        Some(request_id),
                        Some(offered.version.to_string()),
                    ),
                )
                .await?;
            }
            Err(err) => {
                self.fail_request(
                    Some(request_id),
                    map_stage_failure(&err),
                    Some(&err),
                    &format!("update stage failed: {err}"),
                )
                .await?;
            }
        }

        Ok(LoopControl::Continue)
    }

    async fn handle_finalize(
        &self,
        snapshot: &ProtocolSnapshot,
        request_id: String,
    ) -> Result<LoopControl, anyhow::Error> {
        self.patch_protocol(
            &self.config.kubernetes.node_name,
            ProtocolPatch::progress(
                State::Finalizing,
                Some(request_id.clone()),
                snapshot
                    .target_version
                    .clone()
                    .or(snapshot.node_image_version.clone()),
            ),
        )
        .await?;

        let mut client = TridentClient::connect(&self.config.trident.socket).await?;
        match client
            .update_finalize(self.config.orchestration.finalize_timeout)
            .await
        {
            Ok(_) => {
                let finalized_patch = ProtocolPatch::progress(
                    State::Finalized,
                    Some(request_id.clone()),
                    snapshot
                        .target_version
                        .clone()
                        .or(snapshot.node_image_version.clone()),
                );
                let mut patched = false;
                for _ in 0..FINALIZED_PATCH_RETRIES {
                    match self
                        .patch_protocol(&self.config.kubernetes.node_name, finalized_patch.clone())
                        .await
                    {
                        Ok(_) => {
                            patched = true;
                            break;
                        }
                        Err(err) => {
                            log::warn!("failed to publish finalized state before reboot: {err}");
                            tokio::time::sleep(FINALIZED_PATCH_BACKOFF).await;
                        }
                    }
                }
                if !patched {
                    log::warn!(
                        "proceeding to reboot without a confirmed finalized label patch; trident state remains source of truth"
                    );
                }

                match self.rebooter.reboot() {
                    Ok(()) => return Ok(LoopControl::ExitForReboot),
                    Err(err) => {
                        self.fail_request(
                            Some(request_id),
                            FailureReason::FinalizeFailed,
                            None,
                            &format!("finalize succeeded but reboot command failed: {err}"),
                        )
                        .await?;
                    }
                }
            }
            Err(err) => {
                self.fail_request(
                    Some(request_id),
                    map_finalize_failure(&err),
                    Some(&err),
                    &format!("update finalize failed: {err}"),
                )
                .await?;
            }
        }

        Ok(LoopControl::Continue)
    }

    async fn handle_commit(
        &self,
        request_id: Option<String>,
        target_version: Option<String>,
        initial_state: PreviewServicingState,
    ) -> Result<(), anyhow::Error> {
        self.patch_protocol(
            &self.config.kubernetes.node_name,
            ProtocolPatch::progress(
                State::Committing,
                request_id.clone(),
                target_version.clone(),
            ),
        )
        .await?;

        let mut client = TridentClient::connect(&self.config.trident.socket).await?;
        match client
            .commit(self.config.orchestration.finalize_timeout)
            .await
        {
            Ok(_) => {
                self.patch_protocol(
                    &self.config.kubernetes.node_name,
                    ProtocolPatch::progress(State::UpdateSucceeded, request_id, target_version),
                )
                .await?;
            }
            Err(err) => {
                self.fail_request(
                    request_id,
                    map_commit_failure(initial_state, &err),
                    Some(&err),
                    &format!("commit failed: {err}"),
                )
                .await?;
            }
        }

        Ok(())
    }

    async fn fail_request(
        &self,
        request_id: Option<String>,
        reason: FailureReason,
        error: Option<&TridentClientError>,
        message: &str,
    ) -> Result<(), anyhow::Error> {
        let failure_detail = FailureDetail::from_error(request_id.as_deref(), message, error);
        self.patch_protocol(
            &self.config.kubernetes.node_name,
            ProtocolPatch::failed(request_id, reason, failure_detail),
        )
        .await
    }

    async fn patch_protocol(
        &self,
        node_name: &str,
        patch: ProtocolPatch,
    ) -> Result<(), anyhow::Error> {
        let mut labels = BTreeMap::new();
        let mut annotations = BTreeMap::new();

        if let Some(state) = patch.state {
            labels.insert(
                STATE_LABEL.to_string(),
                Some(serde_json::to_string(&state)?.trim_matches('"').to_string()),
            );
        }
        if let Some(request_id) = patch.observed_request_id {
            labels.insert(OBSERVED_REQUEST_ID_LABEL.to_string(), Some(request_id));
        }
        if let Some(reason) = patch.failure_reason {
            labels.insert(
                FAILURE_REASON_LABEL.to_string(),
                Some(
                    serde_json::to_string(&reason)?
                        .trim_matches('"')
                        .to_string(),
                ),
            );
        } else if patch.clear_failure {
            labels.insert(FAILURE_REASON_LABEL.to_string(), None);
            annotations.insert(FAILURE_DETAIL_ANNOTATION.to_string(), None);
        }
        if let Some(node_image_version) = patch.node_image_version {
            labels.insert(
                NODE_IMAGE_VERSION_LABEL.to_string(),
                Some(node_image_version),
            );
        }
        if let Some(detail) = patch.failure_detail {
            annotations.insert(
                FAILURE_DETAIL_ANNOTATION.to_string(),
                Some(serde_json::to_string(&detail)?),
            );
        }

        self.k8s
            .patch_node_metadata(node_name, labels, annotations)
            .await?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProtocolSnapshot {
    request: UpdateRequest,
    request_id: Option<String>,
    target_version: Option<String>,
    state: Option<State>,
    observed_request_id: Option<String>,
    failure_reason: Option<FailureReason>,
    failure_detail: Option<FailureDetail>,
    node_image_version: Option<String>,
}

impl ProtocolSnapshot {
    fn from_node(node: &Node) -> Self {
        let labels = node.metadata.labels.as_ref();
        let annotations = node.metadata.annotations.as_ref();
        let lookup_label = |key: &str| labels.and_then(|values| values.get(key)).cloned();
        let lookup_annotation = |key: &str| annotations.and_then(|values| values.get(key)).cloned();

        Self {
            request: UpdateRequest::parse(lookup_label(REQUEST_LABEL).as_deref())
                .unwrap_or_default(),
            request_id: lookup_label(REQUEST_ID_LABEL),
            target_version: lookup_label(TARGET_VERSION_LABEL),
            state: State::parse(lookup_label(STATE_LABEL).as_deref()),
            observed_request_id: lookup_label(OBSERVED_REQUEST_ID_LABEL),
            failure_reason: FailureReason::parse(lookup_label(FAILURE_REASON_LABEL).as_deref()),
            failure_detail: lookup_annotation(FAILURE_DETAIL_ANNOTATION)
                .and_then(|value| serde_json::from_str::<FailureDetail>(&value).ok()),
            node_image_version: lookup_label(NODE_IMAGE_VERSION_LABEL),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RequestedAction {
    None,
    Reaffirm(State),
    Stage {
        request_id: String,
        target_version: String,
    },
    Finalize {
        request_id: String,
    },
}

fn decide_action(snapshot: &ProtocolSnapshot) -> RequestedAction {
    match snapshot.request {
        UpdateRequest::None => RequestedAction::None,
        UpdateRequest::Stage => match (
            snapshot.request_id.as_ref(),
            snapshot.target_version.as_ref(),
        ) {
            (Some(request_id), Some(target_version)) => {
                if snapshot.observed_request_id.as_deref() == Some(request_id.as_str()) {
                    RequestedAction::Reaffirm(snapshot.state.unwrap_or(State::Ready))
                } else {
                    RequestedAction::Stage {
                        request_id: request_id.clone(),
                        target_version: target_version.clone(),
                    }
                }
            }
            _ => RequestedAction::None,
        },
        UpdateRequest::Finalize => match snapshot.request_id.as_ref() {
            Some(request_id)
                if snapshot.observed_request_id.as_deref() == Some(request_id.as_str())
                    && snapshot.state == Some(State::Staged) =>
            {
                RequestedAction::Finalize {
                    request_id: request_id.clone(),
                }
            }
            Some(request_id)
                if snapshot.observed_request_id.as_deref() == Some(request_id.as_str()) =>
            {
                RequestedAction::Reaffirm(snapshot.state.unwrap_or(State::Ready))
            }
            _ => RequestedAction::None,
        },
    }
}

fn evaluate_version_gate(
    target_version: &str,
    offered_version: Option<&str>,
) -> Result<(), FailureReason> {
    match offered_version {
        None => Err(FailureReason::NoUpdateAvailable),
        Some(offered_version) if offered_version != target_version => {
            Err(FailureReason::VersionMismatch)
        }
        Some(_) => Ok(()),
    }
}

#[derive(Debug, Clone)]
struct ProtocolPatch {
    state: Option<State>,
    observed_request_id: Option<String>,
    failure_reason: Option<FailureReason>,
    failure_detail: Option<FailureDetail>,
    node_image_version: Option<String>,
    clear_failure: bool,
}

impl ProtocolPatch {
    fn progress(
        state: State,
        observed_request_id: Option<String>,
        node_image_version: Option<String>,
    ) -> Self {
        Self {
            state: Some(state),
            observed_request_id,
            failure_reason: None,
            failure_detail: None,
            node_image_version,
            clear_failure: true,
        }
    }

    fn failed(
        observed_request_id: Option<String>,
        failure_reason: FailureReason,
        failure_detail: FailureDetail,
    ) -> Self {
        Self {
            state: Some(State::Failed),
            observed_request_id,
            failure_reason: Some(failure_reason),
            failure_detail: Some(failure_detail),
            node_image_version: None,
            clear_failure: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct FailureDetail {
    message: String,
    trident_error_code: String,
    timestamp: String,
    diagnostics_pointer: String,
}

impl FailureDetail {
    fn from_error(
        request_id: Option<&str>,
        message: &str,
        error: Option<&TridentClientError>,
    ) -> Self {
        let trident_error_code = error
            .and_then(|error| error.remote())
            .map(|remote| remote.subkind.clone())
            .unwrap_or_else(|| "agent-local".to_string());
        let diagnostics_pointer = request_id
            .map(|request_id| {
                format!(
                    "ACL_AbUpdateMetrics/request-id/{request_id}; inspect local trident-metrics.jsonl for correlated diagnostics"
                )
            })
            .unwrap_or_else(|| {
                "ACL_AbUpdateMetrics unavailable; inspect local trident-metrics.jsonl".to_string()
            });

        Self {
            message: message.to_string(),
            trident_error_code,
            timestamp: Utc::now().to_rfc3339(),
            diagnostics_pointer,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoopControl {
    Continue,
    ExitForReboot,
}

fn map_stage_failure(error: &TridentClientError) -> FailureReason {
    match error {
        TridentClientError::Timeout { .. } => FailureReason::Timeout,
        _ if is_volume_mismatch(error) => FailureReason::VolumeMismatch,
        _ if is_download_failure(error) => FailureReason::DownloadFailed,
        _ => FailureReason::StageFailed,
    }
}

fn map_finalize_failure(error: &TridentClientError) -> FailureReason {
    match error {
        TridentClientError::Timeout { .. } => FailureReason::Timeout,
        _ if is_volume_mismatch(error) => FailureReason::VolumeMismatch,
        _ => FailureReason::FinalizeFailed,
    }
}

fn map_commit_failure(
    initial_state: PreviewServicingState,
    error: &TridentClientError,
) -> FailureReason {
    match error {
        TridentClientError::Timeout { .. } => FailureReason::Timeout,
        _ if is_volume_mismatch(error) => FailureReason::VolumeMismatch,
        _ if has_remote_subkind(error, "ab-update-health-check-commit-check") => {
            FailureReason::RollbackSucceeded
        }
        _ if has_remote_subkind(error, "ab-update-reboot-check") => {
            if initial_state == PreviewServicingState::UpdateAbHealthCheckFailed {
                FailureReason::RollbackFailed
            } else {
                FailureReason::RollbackSucceeded
            }
        }
        _ => FailureReason::CommitFailed,
    }
}

fn is_download_failure(error: &TridentClientError) -> bool {
    let Some(remote) = error.remote() else {
        return false;
    };
    remote.subkind == "load-cosi"
        || remote.message.to_ascii_lowercase().contains("download")
        || remote
            .error_message
            .to_ascii_lowercase()
            .contains("download")
}

fn is_volume_mismatch(error: &TridentClientError) -> bool {
    has_remote_subkind(error, "root-device-path-ab-active-volume-mismatch")
        || has_remote_subkind(error, "validate-ab-active-volume")
}

fn has_remote_subkind(error: &TridentClientError, subkind: &str) -> bool {
    error
        .remote()
        .map(|remote| remote.subkind == subkind)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{query_for_update, QueryResult};
    use mockito::Matcher;
    use url::Url;

    #[test]
    fn duplicate_request_id_reaffirms_state() {
        let snapshot = ProtocolSnapshot {
            request: UpdateRequest::Stage,
            request_id: Some("r1".to_string()),
            target_version: Some("202507.28.0".to_string()),
            state: Some(State::Staged),
            observed_request_id: Some("r1".to_string()),
            failure_reason: None,
            failure_detail: None,
            node_image_version: Some("202507.28.0".to_string()),
        };

        assert_eq!(
            decide_action(&snapshot),
            RequestedAction::Reaffirm(State::Staged)
        );
    }

    #[test]
    fn new_request_id_starts_stage_even_after_terminal_state() {
        let snapshot = ProtocolSnapshot {
            request: UpdateRequest::Stage,
            request_id: Some("r2".to_string()),
            target_version: Some("202507.28.0".to_string()),
            state: Some(State::Failed),
            observed_request_id: Some("r1".to_string()),
            failure_reason: Some(FailureReason::StageFailed),
            failure_detail: Some(FailureDetail {
                message: "x".into(),
                trident_error_code: "y".into(),
                timestamp: "z".into(),
                diagnostics_pointer: "d".into(),
            }),
            node_image_version: None,
        };

        assert_eq!(
            decide_action(&snapshot),
            RequestedAction::Stage {
                request_id: "r2".to_string(),
                target_version: "202507.28.0".to_string(),
            }
        );
    }

    #[test]
    fn version_gate_distinguishes_no_update_from_mismatch() {
        assert_eq!(
            evaluate_version_gate("202507.28.0", None).unwrap_err(),
            FailureReason::NoUpdateAvailable
        );
        assert_eq!(
            evaluate_version_gate("202507.28.0", Some("202508.1.0")).unwrap_err(),
            FailureReason::VersionMismatch
        );
        assert!(evaluate_version_gate("202507.28.0", Some("202507.28.0")).is_ok());
    }

    #[test]
    fn mockito_stage_queries_cover_no_update_and_mismatch() {
        let mut server = mockito::Server::new();

        let no_update = server
            .mock("POST", "/noupdate")
            .match_body(Matcher::Regex(".*<updatecheck.*".to_string()))
            .with_status(200)
            .with_body(indoc::indoc! {r#"
                <?xml version="1.0" encoding="UTF-8"?>
                <response protocol="3.0" server="mock">
                    <daystart elapsed_seconds="0"/>
                    <app appid="test" status="ok">
                        <updatecheck status="noupdate">
                            <urls></urls>
                        </updatecheck>
                    </app>
                </response>
            "#})
            .create();

        let mismatch = server
            .mock("POST", "/mismatch")
            .match_body(Matcher::Regex(".*<updatecheck.*".to_string()))
            .with_status(200)
            .with_body(format!(
                indoc::indoc! {r#"
                <?xml version="1.0" encoding="UTF-8"?>
                <response protocol="3.0" server="mock">
                    <daystart elapsed_seconds="0"/>
                    <app appid="test" status="ok">
                        <updatecheck status="ok">
                            <urls><url codebase="{}"/></urls>
                            <manifest version="202508.1.0">
                                <packages>
                                    <package hash="ignored" name="acl.cosi" size="1" required="true"/>
                                </packages>
                            </manifest>
                        </updatecheck>
                    </app>
                </response>
                "#},
                server.url()
            ))
            .create();

        let response = query_for_update(
            &Url::parse(&format!("{}/noupdate", server.url())).unwrap(),
            "test",
            "track",
            &Version::new(0, 0, 0),
            IdSource::MachineIdHashed,
        )
        .unwrap();
        assert!(matches!(response.result, QueryResult::NoUpdate));
        assert_eq!(
            evaluate_version_gate("202507.28.0", None).unwrap_err(),
            FailureReason::NoUpdateAvailable
        );

        let response = query_for_update(
            &Url::parse(&format!("{}/mismatch", server.url())).unwrap(),
            "test",
            "track",
            &Version::new(0, 0, 0),
            IdSource::MachineIdHashed,
        )
        .unwrap();
        let offered = match response.result {
            QueryResult::NewDocument(update) => update.version.to_string(),
            QueryResult::NoUpdate => panic!("expected offered update"),
        };
        assert_eq!(
            evaluate_version_gate("202507.28.0", Some(&offered)).unwrap_err(),
            FailureReason::VersionMismatch
        );

        no_update.assert();
        mismatch.assert();
    }
}
