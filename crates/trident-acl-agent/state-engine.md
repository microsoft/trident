# Trident ACL Agent: A/B Update State Engine

This document summarizes the actual, as-implemented interactions between the
five participants in an AKS A/B update: **aks-rp**, the **Kubernetes API
server**, **trident-acl-agent** (Harpoon), **Nebraska/Omaha**, and **tridentd**.
It reflects the code in this crate (`orchestrator.rs`, `labels.rs`, `k8s.rs`,
`trident.rs`, `config.rs`), not just the design intent — see
[`design-decisions.md`](./design-decisions.md) in this crate for the original
design rationale, the full decision log, and the open questions this
implementation resolves with documented assumptions.

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
     handles a crash/restart between stage and finalize, and also downgrades
     a stale `finalizing` label back to `staged` if the crash happened
     mid-finalize before tridentd's own state advanced — `decide_action` then
     re-drives `handle_finalize` on the next reconcile.
   - If `ServicingState == UpdateAbFinalized`: **this state is ambiguous** —
     it's reached both by a genuine post-reboot resume *and* by the agent
     crashing/restarting after a successful finalize but before (or during)
     its own `reboot()` call, i.e. still running on the pre-update boot. The
     two are distinguished via `tridentd.get_last_error()`: no last error
     means finalize completed but the reboot never actually happened (a bare
     restart), so the agent re-issues `reboot()` rather than committing; a
     last error present means a real reboot occurred (typically from the
     health-check/reboot path) and the agent proceeds to `handle_commit`.
   - If `ServicingState == UpdateAbHealthCheckFailed`: this state is only
     reachable after the host has actually booted into the target OS and
     health checks ran, so unlike `UpdateAbFinalized` it unambiguously implies
     a real reboot — proceed straight to `handle_commit`.
   - If `ServicingState == Provisioned` and the label snapshot's `state` is
     still `staging`/`finalizing`/`committing` for the current request-id:
     that in-flight operation was interrupted by a reboot the agent itself
     didn't drive to completion (or a rollback) and cannot be resumed —
     `fail_request(Timeout)` so the RP can retry with a fresh request-id,
     rather than leaving a dangling in-flight label.
   - Any other state (`NotProvisioned`/`InstallStaged`/`InstallFinalized`,
     etc.): if the label snapshot's `state` is still `staging`/`finalizing`/
     `committing`, this means the agent crashed *before* tridentd's own state
     ever advanced past where it started (e.g. crashed just after patching
     `state=staging` but before the `UpdateStage` RPC ran, or crashed mid
     Nebraska query). There is no in-progress tridentd operation to resume,
     so the request is failed explicitly (`fail_request(Timeout)`) instead of
     leaving the stale label in place. Otherwise, `ensure_ready_label` only.

This ordering is the concrete implementation of the design's "Trident's own
state is the source of truth, not labels" decision — labels are a status
mirror the agent maintains, never an input trusted blindly across a restart
boundary. Explicitly failing (rather than silently reaffirming) any
transitional label state that startup recovery cannot corroborate against
`tridentd`'s own state is what closes the "permanent silent stall" gap found
in deep review (see below).

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

1. **Auth**: `NodeClient::new`'s explicit-kubeconfig-path branch is
   **identity-agnostic** — it loads whatever kubeconfig `[kubernetes].kubeconfig`
   points at with no check on which credential it contains, so nothing in
   this crate stops an operator from pointing it at kubelet's own kubeconfig
   (e.g. `/var/lib/kubelet/kubeconfig`, the example value used in
   `design-decisions.md`'s config schema). Only the *fallback* branch — used
   when no `kubeconfig` path is configured, or the configured path doesn't
   exist — carries a comment assuming a dedicated ServiceAccount identity via
   `Config::infer()`. This is therefore **not a resolved decision**: it is an
   unenforced deployment recommendation, and the underlying question —
   whether the agent may reuse kubelet's identity, or must use a dedicated
   scoped credential — remains open pending security sign-off (see
   [`design-decisions.md`](./design-decisions.md), decision #7). RBAC
   manifests are out of this crate's scope regardless of which identity model
   is chosen.
2. **Activation**: label-mode is opt-in via `[orchestration].goal_source =
   "labels"` in the config file only; default is `"omaha-only"` (inactive).
3. **Timeouts**: `stage_timeout`/`finalize_timeout` default to `20m`/`10m`
   placeholders, configurable — real values are expected to come from
   `storm aclagent` scenario runs.
4. **Duplicate request-id**: treated as an idempotent re-affirmation
   (`RequestedAction::Reaffirm`), never a restart of in-flight or completed
   work.
5. **`no-update-available`**: a distinct `FailureReason`, separate from hard
   stage failures, for the case where Nebraska simply hasn't published the
   requested version yet.
6. **Reboot vs. bare process restart**: `tridentd`'s `UpdateAbFinalized`
   servicing state is ambiguous on its own — it's reported both immediately
   after a successful finalize (before any reboot) and after a genuine
   post-reboot resume. `recover_from_trident_state` disambiguates via
   `tridentd.get_last_error()`: no last error means the agent restarted
   without the machine rebooting (finalize completed but reboot didn't
   happen or didn't take effect), so the agent re-issues `reboot()`; a last
   error present means a real reboot occurred, so the agent proceeds to
   `commit()`. `UpdateAbHealthCheckFailed` and `Provisioned` are treated as
   unambiguous reboot evidence and need no such check.

## Known follow-ups (tracked, not blocking)

- **Commit safety on tridentd's side**: whether `tridentd`'s own `commit()`
  handler independently verifies it is running from the newly-finalized root
  (e.g. via boot-loader/active-volume state) is outside this crate — the
  agent-side fix above closes the *reachable* restart-vs-reboot ambiguity,
  but a defense-in-depth check inside `tridentd` itself was not evaluated as
  part of this change.
- **`watch_node` naming**: currently a poll loop (`k8s.rs`), not a true K8s
  watch; works correctly but the name may mislead readers expecting
  watch-semantics (e.g. server-side filtering, no polling interval).
