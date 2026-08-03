# Trident ACL Agent: A/B Update State Engine

This document summarizes the actual, as-implemented interactions between the
five participants in an AKS A/B update: **aks-rp**, the **Kubernetes API
server**, **trident-acl-agent** (Harpoon), **Nebraska/Omaha**, and **tridentd**.
It reflects the code in this crate (`orchestrator.rs`, `labels.rs`, `k8s.rs`,
`trident.rs`, `config.rs`), not just the design intent — see
`wiki/projects/acl-aks/trident-acl-agent-label-protocol.md` in the `mjolnir`
wiki for the original design rationale and open questions this implementation
resolves with documented assumptions.

This orchestration path only runs when `[orchestration].goal_source = "labels"`
in the agent's config file. The default is `"omaha-only"`, which preserves the
original one-shot Harpoon behavior and never touches Node labels.

## Participants

| Participant | Role |
|---|---|
| **aks-rp** | Writes desired state (`request`, `request-id`, `target-os-image-version`) to the Node; watches for terminal state. Not part of this crate — modeled here only by the labels it reads/writes. |
| **API server** | Holds the Node object. All communication between aks-rp and the agent is mediated through Node labels/annotations, patched via `NodeClient` (`k8s.rs`). |
| **trident-acl-agent** | This crate. Watches its own Node, drives Nebraska queries and `tridentd` RPCs, and reports progress back via label patches. |
| **Nebraska/Omaha** | Content channel. Returns an image URL (+ hash) and version for a given app/track — never the label protocol's concern beyond the version gate. |
| **tridentd** | Owns the actual stage/finalize/commit mechanics via gRPC (`UpdateService`, `CommitService`) and exposes `ServicingState` for crash-safe resume. |

## Label/annotation schema (`labels.rs`)

RP → Agent (desired state):

| Label | Values |
|---|---|
| `kubernetes.azure.com/trident-abupdate-request` | `stage` \| `finalize` \| `none` |
| `kubernetes.azure.com/trident-abupdate-request-id` | opaque token |
| `kubernetes.azure.com/trident-abupdate-target-os-image-version` | e.g. `202507.28.0` |

Agent → RP (observed state):

| Label | Values |
|---|---|
| `kubernetes.azure.com/trident-abupdate-state` | `ready` \| `staging` \| `staged` \| `finalizing` \| `finalized` \| `committing` \| `update-succeeded` \| `failed` |
| `kubernetes.azure.com/trident-abupdate-observed-request-id` | echoes the request-id acted on |
| `kubernetes.azure.com/trident-abupdate-failure-reason` | see below |
| `kubernetes.azure.com/node-image-version` | reused AgentBaker label, refreshed on `staged`/`update-succeeded` |

Annotation: `kubernetes.azure.com/trident-abupdate-failure-detail` — JSON
(`FailureDetail`) with message, underlying `TridentClientError` info if any,
and the request-id, written whenever `state=failed`.

Failure reasons (`FailureReason`): `download-failed`, `stage-failed`,
`version-mismatch`, `finalize-failed`, `commit-failed`, `volume-mismatch`,
`timeout`, `rollback-succeeded`, `rollback-failed`, `no-update-available`.

## Startup: recovering from `tridentd` before trusting labels

On every process start (including post-reboot), `recover_from_trident_state()`
runs **before** the watch loop begins:

1. Fetch the current Node object and build a `ProtocolSnapshot` from its
   labels/annotations (this is a read-only view; labels are not yet trusted).
2. Connect to `tridentd` and call `get_servicing_state()`.
   - If `tridentd` is unreachable or errors: log a warning and fall back to
     `ensure_ready_label` (only patches `state=ready` if no request is
     outstanding — never overwrites an in-flight request's labels blindly).
   - If `ServicingState == UpdateAbStaged`: patch `state=staged`, carrying
     forward whatever request-id/target-version the snapshot had. This
     handles a crash/restart between stage and finalize.
   - If `ServicingState` is `UpdateAbFinalized` or `UpdateAbHealthCheckFailed`:
     go straight to `handle_commit(...)` — this is the **primary post-reboot
     resume path**, since a successful finalize+reboot lands here.
   - Any other state: `ensure_ready_label` only.

This ordering is the concrete implementation of the design's "Trident's own
state is the source of truth, not labels" decision — labels are a status
mirror the agent maintains, never an input trusted blindly across a restart
boundary.

## Steady-state loop: reacting to label changes

After recovery, `run()` watches the agent's own Node (`k8s.watch_node`) and
calls `reconcile_node` on every observed change. `decide_action` classifies
the current label snapshot into one of:

- **`None`** — no `request` label (or `request=none`): `ensure_ready_label`
  only sets `state=ready` if there is no `state` label yet (idempotent no-op
  once ready).
- **`Reaffirm(state)`** — the observed `request-id` matches
  `observed-request-id` already recorded (i.e. this is a retry/duplicate of a
  request already acted on, or already handled). The agent **does not restart
  the operation**; it just re-patches the current terminal/in-flight state so
  a retried RP PATCH doesn't trigger duplicate work. *(Documented assumption
  resolving the design doc's open "RP retry" question.)*
- **`Stage { request_id, target_version }`** — a genuinely new stage request
  (new `request-id`, or `request=stage` with no matching `observed-request-id`
  yet) → `handle_stage`.
- **`Finalize { request_id }`** — → `handle_finalize`.

### `handle_stage`

1. Patch `state=staging`, `observed-request-id=<request-id>`.
2. Query Nebraska (`query_for_update`) using the configured endpoint/app-id.
   - `QueryResult::NoUpdate` → `fail_request(NoUpdateAvailable)`. This is
     deliberately distinct from a hard failure: it usually means RP raced
     ahead of Nebraska publishing the target version. *(Resolves the design
     doc's open question on this exact ambiguity.)*
   - `QueryResult::NewDocument(offered)` → proceed to the version gate.
3. **Version gate** (`evaluate_version_gate`): if `offered.version !=
   target_version`, `fail_request(VersionMismatch)` — the agent never stages
   a version the RP didn't ask for.
4. On match: connect to `tridentd`, call `update_stage(url, hash,
   stage_timeout)`.
   - Success → patch `state=staged`, refresh `node-image-version` to the
     offered version.
   - Failure → `fail_request` with a reason from `map_stage_failure` (e.g.
     `download-failed`/`stage-failed`), including whatever `TridentClientError`
     detail is available in the failure-detail annotation.

### `handle_finalize`

1. Patch `state=finalizing`.
2. Call `tridentd.update_finalize(finalize_timeout)` — this uses
   `CallerHandlesReboot`, i.e. `tridentd` does **not** reboot on its own.
3. On success: attempt to patch `state=finalized` **before** rebooting, with
   bounded retries (`FINALIZED_PATCH_RETRIES = 3`, 2s backoff). If all
   retries fail, proceed to reboot anyway — the label write is best-effort;
   `tridentd`'s own state remains authoritative and will be re-derived by the
   next startup's `recover_from_trident_state()`.
4. The agent itself invokes the reboot (`RebootHandle::reboot`, trying
   `reboot` then `systemctl reboot`), then returns `LoopControl::ExitForReboot`
   to end the watch loop cleanly (the process is expected to exit and be
   restarted by its supervisor after the machine comes back up).
5. If finalize itself fails: `fail_request` with `map_finalize_failure`'s
   reason (typically `finalize-failed`).
6. If finalize succeeds but the reboot command itself fails to invoke:
   `fail_request(FinalizeFailed)` — this is a distinct, atypical failure mode
   (finalize genuinely succeeded, but the machine never went down), flagged
   for operator attention rather than silently retried.

### `handle_commit` (post-reboot path)

Reached only from `recover_from_trident_state` when `tridentd` reports
`UpdateAbFinalized`/`UpdateAbHealthCheckFailed` on startup:

1. Patch `state=committing`.
2. Call `tridentd.commit(finalize_timeout)`.
   - Success → patch `state=update-succeeded`, refresh `node-image-version`.
   - Failure → `fail_request` with `map_commit_failure(initial_state, err)` —
     distinguishes `commit-failed` vs `rollback-succeeded` vs
     `rollback-failed` depending on what `tridentd` actually did.

`update-succeeded` and `failed` are terminal for a given `request-id`; a new
`request-id` from the RP restarts the cycle from `Stage`/`Finalize`
classification in `decide_action`.

## End-to-end sequence (nominal path)

```
aks-rp          API server        trident-acl-agent         Nebraska        tridentd
  |--PATCH stage,-->|                    |                       |               |
  |  req-id=R1       |<--watch event-----|                       |               |
  |                 |                    |--query update-------->|               |
  |                 |                    |<--url, hash, version--|               |
  |                 |<--PATCH staging----|                       |               |
  |                 |                    |--UpdateStage(config)---------------->|
  |                 |                    |<--stream: Started/Log/Completed------|
  |                 |<--PATCH staged-----|                       |               |
  |--sees staged; drains node (outside this protocol)--          |               |
  |--PATCH finalize->|                    |                       |               |
  |                 |<--watch event------|                       |               |
  |                 |<--PATCH finalizing-|                       |               |
  |                 |                    |--UpdateFinalize(CallerHandlesReboot)->|
  |                 |                    |<--Completed{success}------------------|
  |                 |<--PATCH finalized--| (best-effort, bounded retry)          |
  |                 |                    |--reboot()--                           |
  |                 |                    (process exits; machine reboots)        |
  |                 |                    |--restart; get_servicing_state()------>|
  |                 |                    |<--UpdateAbFinalized-------------------|
  |                 |<--PATCH committing-|                       |               |
  |                 |                    |--Commit()------------------------------>|
  |                 |                    |<--Completed{success}--------------------|
  |                 |<--PATCH update-----|                       |               |
  |                 |    succeeded       |                       |               |
  |--watches for observed-request-id=R1 AND state in             |               |
  |  {update-succeeded, failed}; operation complete               |               |
```

## Assumptions this implementation bakes in

These resolve design-doc open questions with concrete engineering choices —
flagged here for reviewer visibility, not because they're beyond challenge:

1. **Auth**: the agent authenticates via its own client config
   (`NodeClient::new`), not by reusing kubelet's node identity — see
   `config.rs`/`k8s.rs`. RBAC manifests themselves are out of this crate's
   scope (deployment-time concern).
2. **Activation**: label-mode is opt-in via `[orchestration].goal_source =
   "labels"` in the config file only; default is `"omaha-only"` (inactive).
3. **Timeouts**: `stage_timeout`/`finalize_timeout` default to `20m`/`10m`
   placeholders, configurable — real values are expected to come from
   `trident-acl-agent-tester` scenario runs.
4. **Duplicate request-id**: treated as an idempotent re-affirmation
   (`RequestedAction::Reaffirm`), never a restart of in-flight or completed
   work.
5. **`no-update-available`**: a distinct `FailureReason`, separate from hard
   stage failures, for the case where Nebraska simply hasn't published the
   requested version yet.
