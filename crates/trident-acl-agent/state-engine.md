# Trident ACL Agent: Annotation Protocol State Engine

This document describes the accepted-design-based implementation in
`crates/trident-acl-agent/` as currently implemented.

## Protocol surface

The agent now uses exactly two Node annotations:

- `acl.azure.com/update-request`
- `acl.azure.com/update-status`

`update-request` carries `schemaVersion`, `nodeUpdateId`, `operationId`,
`operation`, and `targetVersion` (only for stage/finalize).

`update-status` carries the same correlation fields plus `code`, `message`,
`fromVersion`, `toVersion`, `startedUtc`, and `finishedUtc`.

The Rust schema lives in `src/annotations.rs` and uses serde with
`deny_unknown_fields` to keep the payload shape tight.

## Operations

Implemented operations:

- `stage`
- `finalize`
- `rollback`
- implicit post-reboot `commit`

`stage` and `finalize` use the existing gRPC wrapper in `src/trident.rs`.
`rollback` shells out to the `trident` CLI:

- `trident rollback --ab --allowed-operations=stage`
- `trident rollback --allowed-operations=finalize`

`commit` is never requested by AKS-RP. The agent resumes it from persisted
state after reboot.

## Persistent state

The agent persists `state.json` at the configured
`[orchestration].state_path` (default `/var/lib/trident-acl-agent/state.json`).
The file schema is implemented in `src/state.rs`:

```json
{
  "pendingCommit": { ... } | null,
  "completed": {
    "<operationId>": { ...terminal status... }
  }
}
```

`pendingCommit` is written before finalize/rollback reboots. `completed`
caches terminal payloads so retries re-serve the same status instead of
re-running work.

## State machine

### Stage

1. Parse request annotation.
2. If current active version already equals `targetVersion`, emit
   `AlreadyAtTarget`.
3. Publish `InProgress`.
4. Query Nebraska and validate offered version.
5. Call `update_stage()`.
6. Emit terminal `Success`, `OperationFailed`, or `InvalidRequest`.

### Finalize

1. Require a cached successful `stage` status for the same `nodeUpdateId`.
2. Short-circuit `AlreadyAtTarget` if already on target.
3. Otherwise emit `NotStaged` if no prior successful stage exists.
4. Publish `InProgress` and persist `pendingCommit`.
5. Call `update_finalize()` with CallerHandlesReboot.
6. Publish terminal finalize status best-effort, then reboot.
7. On next startup, resume `commit` from `pendingCommit`.

### Rollback

1. Publish `InProgress`.
2. Run rollback stage via CLI.
3. Persist `pendingCommit`.
4. Run rollback finalize via CLI, which reboots.
5. Resume with implicit `commit` after startup.

### Commit

At startup, if `pendingCommit` exists, the agent:

1. Publishes `commit/InProgress` using `<operationId>.commit`.
2. Calls `commit()` with CallerHandlesReboot.
3. Emits `Success`, `RevertedToPrevious`, `OperationFailed`, or
   `AgentInternalError`.
4. Clears `pendingCommit` and records the terminal payload in `completed`.

## Concurrency policy

If `pendingCommit` exists and a new request arrives with a different
`nodeUpdateId`, the agent rejects it with `InvalidRequest`.

## Kubeconfig identity

`src/k8s.rs` now always loads `/var/lib/kubelet/kubeconfig` (or the explicit
configured path, which defaults there) instead of falling back to inferred
cluster identity.

## Degraded reconstruction when `state.json` is missing

The current implementation provides a minimal degraded path:

- if the stubbed active version matches finalize target, report commit success
- otherwise emit `AgentInternalError`
- commit-side gRPC subkinds `ab-update-reboot-check` and
  `ab-update-health-check-commit-check` map to `RevertedToPrevious`

This is intentionally incomplete relative to the ideal design because the
needed durable boot-history/rollback-chain signal is not exposed through the
current agent-facing wrapper.

## Assumptions / open items

1. **Current active version**: `fromVersion` currently comes from a hardcoded
   stub (`"202601.1.0"`) in `src/annotations.rs`. This is a stopgap standing
   in for a future `/etc`-based probe or a future request-side `fromVersion`
   contract from AKS-RP.
2. **Accepted design source doc**: the repo snapshot used for this work did not
   contain `accepted-design.md`; implementation followed the task contract and
   preserved the explicitly-requested behaviors.
3. **Degraded reconstruction**: without a surviving `state.json`, the agent
   cannot yet distinguish every boot-history case described in the target
   design. That remains a follow-up once Trident exposes the necessary signal.
