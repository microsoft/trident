# Update Trigger - Design Document

AKS needs a way to tell a specific ACL node when to A/B update, and to learn what
happened.
This design uses annotations. The pieces are a pair of annotations on the Node object, an
in-image agent that reads and writes them, a state file that bridges the partition swap, a
watchdog AKS operates as a backstop, and a new branch in AKS's per-node upgrade loop that
drives the annotations instead of CRP.

Two alternatives were considered and rejected. A pod-based cluster extension needs kubelet
to be up, which is not guaranteed during an upgrade window or post-reboot. A manifest-only
VM extension keeps waagent on the trigger path, which runs against the ACL image's hardening
posture of minimizing waagent dependence.

## 1. Requirements

- P0: AKS-RP owns every decision. The node reports what happened and waits; it never decides
on its own to roll forward, roll back, or return to service (see 2.5).
- P0: AKS-RP can target a specific ACL node and request one of `stage`, `finalize`, or
`rollback` (names mirror Trident's API, see 2.1).
- P0: AKS-RP learns the outcome of each request as one of a fixed set of result codes.
- P0: A `finalize` (or `rollback`) produces two terminal statuses: one for `finalize`
itself, written before the reboot, and a follow-up `commit` status written after the new
partition boots and Trident validates the booted volume. AKS-RP waits for both.
- P0: Re-issuing the same `operationId` returns the cached terminal status without redoing
the work.
- P0: Silent failures (any case the agent fails to write a terminal status) are visible to
AKS-RP as "no terminal status within SLA", so the watchdog can reprovision the node.
- P1: The request and status annotations carry a `schemaVersion` field and conform to a
formal JSON Schema (2.1) so AKS-RP and the agent can evolve the contract independently and
validate payloads at both ends.
- P1: System health is observable by logging each terminal `code` per operation, so AKS-RP
can alert when non-`Success` results spike.
- P1: Performance is observable by tracking per-node downtime and per-node rollback
duration, so we can quantify how much time was saved with A/B updates.

## 2. Design

### 2.1 Annotation contract

AKS-RP triggers an update by writing a JSON payload to one annotation on the target Node
object.
The agent writes its progress and result back to a second annotation on the same Node.
Operation names mirror [Trident's CLI](https://microsoft.github.io/trident/docs/Reference/Trident-CLI/)
phases, with the exact mapping shown in the operations table below.

**Request annotation** at `acl.azure.com/update-request`:

```json
{
  "schemaVersion": "1.0",
  "nodeUpdateId":  "550e8400-e29b-41d4-a716-446655440000",
  "operationId":   "f47ac10b-58cc-4372-a567-0e02b2c3d479",
  "operation":     "finalize",
  "targetVersion": "202606.29.0"
}
```

`targetVersion` is the ACL image release version, which AKS-RP already has.
It is required for `stage` and `finalize`; for `rollback` the target
is implicit (the previous partition), so it is omitted.

`nodeUpdateId` is a UUID identifying one node's update sequence, and is set by AKS-RP once
per node update and held constant across the `stage` -> `finalize` -> `commit`
steps.
A retry that starts over from `stage` gets a fresh `nodeUpdateId`.

`operationId` is a UUID identifying this specific step.
The agent compares `operationId` against the last one it saw to decide whether to start
new work, resume in-flight work, or re-emit a cached terminal status as a no-op.

`operation` is one of three values AKS-RP can request:

| `operation` | other fields | meaning | Trident invocation |
|---|---|---|---|
| `stage` | `targetVersion` | Stage the target image on the inactive partition. No reboot. | [`trident update --allowed-operations=stage`](https://microsoft.github.io/trident/docs/Reference/Trident-CLI/#update) |
| `finalize` | `targetVersion` | Arm boot config for the staged target. The gRPC `UpdateFinalize` is called with `RebootManagement = CALLER_HANDLES_REBOOT`, so Trident arms boot and returns `Completed` (reboot required) without rebooting; the agent writes the terminal `finalize` status first, then triggers the reboot as a separate step (see 2.3). Terminal `Success` means boot was armed and the reboot is about to happen, not that the new image is good. | [`trident update --allowed-operations=finalize`](https://microsoft.github.io/trident/docs/Reference/Trident-CLI/#update) (gRPC `UpdateFinalize`, caller-handled reboot) |
| `rollback` | (target is implicit: the previous partition) | AKS-RP-initiated **partition-swap** back to the previous partition, mirroring `finalize` on the return path. Only the *last* A/B update can be undone this way. Its trigger and scope are covered in 2.5. | [`trident rollback --ab`](https://microsoft.github.io/trident/docs/Explanation/Manual-Rollback/) (gRPC `RollbackStage` then `RollbackFinalize`, caller-handled reboot — see 2.3) |

A fourth phase, `commit`, runs implicitly post-reboot after `finalize` or `rollback`:
the agent does not need AKS-RP to PATCH a `commit` request, it just runs
[`trident commit`](https://microsoft.github.io/trident/docs/Reference/Trident-CLI/#commit)
on the new partition and writes a status annotation for it.
AKS-RP watches for this status as the terminal signal that the reboot half succeeded.

`commit` validates that the node booted the expected volume and promotes the boot order.
The `BootNext` / `BootOrder` mechanics, Trident's optional health checks, and how a failed
update is recovered are all covered in 2.5.

**Status annotations.** The agent writes two status keys. `acl.azure.com/update-status`
holds the status of the requested operation. `acl.azure.com/update-commit-status` holds the
status of the implicit post-reboot `commit`. Each key has one writer and one meaning, so a
`commit` status never overwrites the `finalize` status that came before it.

**Operation status** at `acl.azure.com/update-status`:

```json
{
  "schemaVersion": "1.0",
  "nodeUpdateId":  "550e8400-e29b-41d4-a716-446655440000",
  "operationId":   "f47ac10b-58cc-4372-a567-0e02b2c3d479",
  "operation":     "finalize",
  "code":          "Success",
  "message":       "boot armed, rebooting, awaiting commit",
  "fromVersion":   "202605.15.0",
  "toVersion":     "202606.29.0",
  "startedUtc":    "2026-06-04T12:00:00Z",
  "lastUpdatedUtc": "2026-06-04T12:00:32Z",
  "finishedUtc":   "2026-06-04T12:00:32Z"
}
```

**Commit status** at `acl.azure.com/update-commit-status`. After the reboot, the agent
writes the `commit` status to this second key. The payload keeps the `nodeUpdateId` and the
`operationId` of the `finalize` or `rollback` that caused the reboot, so AKS-RP correlates
the two halves without an extra round-trip:

```json
{
  "schemaVersion": "1.0",
  "nodeUpdateId":  "550e8400-e29b-41d4-a716-446655440000",
  "operationId":   "f47ac10b-58cc-4372-a567-0e02b2c3d479",
  "operation":     "commit",
  "code":          "Success",
  "message":       "booted expected volume, boot order promoted",
  "fromVersion":   "202605.15.0",
  "toVersion":     "202606.29.0",
  "startedUtc":    "2026-06-04T12:01:18Z",
  "lastUpdatedUtc": "2026-06-04T12:01:32Z",
  "finishedUtc":   "2026-06-04T12:01:32Z"
}
```

`code` is one of:

| `code` | meaning | terminal? |
|---|---|---|
| `InProgress` | Request received, work in progress. The agent refreshes `lastUpdatedUtc` on a heartbeat while the work runs, so a stalled agent is distinguishable from a slow operation. | no |
| `Success` | The operation completed. For `stage`, the image is on the inactive partition. For `finalize` / `rollback`, boot has been armed and the reboot is about to be triggered; AKS-RP is still waiting for the follow-up `commit` status. For `commit`, the node booted the expected volume and Trident promoted the boot order. | yes |
| `AlreadyAtTarget` | The node is already on `targetVersion`, nothing to do. | yes |
| `NotStaged` | `finalize` issued without a prior matching `stage`. | yes |
| `OperationFailed` | The operation could not be completed: the agent could not resolve `targetVersion` to a downloadable image, or Trident failed to stage, verify, or arm boot for it. The node is untouched and still on its current version. | yes |
| `TargetBootFailed` | The target OS failed to boot, so the firmware started the previous version again. Nothing was promoted, so the update never took effect. The node stays cordoned until AKS-RP returns it to service. Only appears on a `commit` status. See 2.5. | yes |
| `AgentInternalError` | The agent itself crashed or hit an unexpected error. | yes |
| `InvalidRequest` | The annotation payload did not parse, the operation is not supported, or the request conflicts with an operation already in flight (see the concurrency note below). | yes |

AKS-RP matches `operationId` on each status annotation against the request it last issued.
The `commit` status carries the same `operationId` on its own key, so the key tells AKS-RP
which half it reads. Any other `operationId` is ignored as stale.

The agent caps `message` at 2048 bytes and marks a message that it cut. A Node object
holds at most 256 KB across all of its annotations, and AKS already sets many of them, so
an uncapped error dump can exhaust what is left. The cap has to be applied by the writer.
AKS-RP also truncates an over-long message when it reads one, but by then the write has
already landed, so that only protects AKS-RP's own logs.

End-to-end, the mechanism is one annotation PATCH out, one (for `stage`) or two
(for `finalize` / `rollback`) annotation watches back, with a watchdog as a backstop.

`stage`:

```mermaid
sequenceDiagram
    actor AKSRP as AKS-RP
    participant API as K8s API Server
    participant Agent as Trident ACL Agent<br/>(on the node)
    participant Trident

    AKSRP->>API: 1. PATCH request: stage (opId A)
    Note over Agent: agent picks up the request<br/>(next timer tick, or immediately if daemon)
    Agent->>API: 2. read request, PATCH status: stage InProgress
    Agent->>Trident: 3. Stage (image to inactive partition)
    Trident-->>Agent: staged | error
    Agent->>API: 4. PATCH status: stage Success | <error code>
    API-->>AKSRP: terminal stage code
```

`finalize` / `rollback`:

```mermaid
sequenceDiagram
    actor AKSRP as AKS-RP
    participant API as K8s API Server
    participant Agent as Trident ACL Agent<br/>(on the node)
    participant Trident

    Note over AKSRP,Trident: pre-reboot half
    AKSRP->>API: 1. PATCH request: finalize (opId A)
    Note over Agent: agent picks up the request<br/>(next timer tick, or immediately if daemon)
    Agent->>API: 2. read request, PATCH status: finalize InProgress
    Agent->>Trident: 3. UpdateFinalize (CALLER_HANDLES_REBOOT)
    Trident-->>Agent: boot armed, reboot required
    Agent->>Agent: 4. persist pendingCommit + boot marker to state.json
    Agent->>API: 5. PATCH status: finalize Success
    API-->>AKSRP: finalize Success (reboot pending)
    Note over Agent,Trident: 6. agent triggers reboot, boots new partition
    Note over AKSRP,Trident: post-reboot half
    Agent->>Agent: 7. read state.json, confirm a boot happened since the marker
    Agent->>Trident: 8. Commit (validate volume, promote boot order)
    Trident-->>Agent: committed | reverted to previous
    Agent->>API: 9. PATCH status: commit Success | TargetBootFailed
    API-->>AKSRP: terminal commit code
    Note over AKSRP: no commit status within SLA, watchdog reprovisions
```

If AKS-RP re-PATCHes the same `operationId` while the agent is mid-work, the next agent
invocation matches `pendingCommit` and resumes the post-reboot commit, or for `stage`,
re-runs the idempotent work and re-emits `InProgress` until it finishes.
Either way, the outcome is the same as if the duplicate PATCH had never been sent.

However, if AKS-RP PATCHes a different `operationId` while a finalize is in flight, the
agent has no good answer: it can't cancel a reboot mid-flight, and the post-reboot half
will read the request annotation and find the new `operationId` instead of the one
it was finalizing for.
Proposal: reject the new request with `InvalidRequest` and let AKS-RP wait for the `commit`
terminal status before issuing the next operation.

**Formal JSON Schema**

Both annotation payloads conform to the following JSON Schemas, so AKS-RP and
the agent can validate at both ends and evolve the contract independently via
`schemaVersion`.

Request (`acl.azure.com/update-request`):

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://acl.azure.com/schemas/update-request/1.0.json",
  "title": "ACL A/B update request annotation",
  "type": "object",
  "additionalProperties": false,
  "required": ["schemaVersion", "nodeUpdateId", "operationId", "operation"],
  "properties": {
    "schemaVersion": { "type": "string", "const": "1.0" },
    "nodeUpdateId":  { "type": "string", "format": "uuid", "pattern": "^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$" },
    "operationId":   { "type": "string", "format": "uuid", "pattern": "^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$" },
    "operation":     { "type": "string", "enum": ["stage", "finalize", "rollback"] },
    "targetVersion": { "type": "string", "description": "ACL image release version, e.g. 202606.29.0." }
  },
  "allOf": [
    {
      "if":   { "properties": { "operation": { "enum": ["stage", "finalize"] } }, "required": ["operation"] },
      "then": { "required": ["targetVersion"] }
    }
  ]
}
```

Status (`acl.azure.com/update-status`):

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://acl.azure.com/schemas/update-status/1.0.json",
  "title": "ACL A/B update status annotation",
  "type": "object",
  "additionalProperties": false,
  "required": ["schemaVersion", "nodeUpdateId", "operationId", "operation", "code"],
  "properties": {
    "schemaVersion": { "type": "string", "const": "1.0" },
    "nodeUpdateId":  { "type": "string", "format": "uuid", "pattern": "^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$" },
    "operationId":   { "type": "string", "format": "uuid", "pattern": "^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$", "description": "The operationId of the request this status reports on. The post-reboot commit status repeats the operationId of the finalize or rollback that caused the reboot." },
    "operation":     { "type": "string", "enum": ["stage", "finalize", "rollback", "commit"] },
    "code":          { "type": "string", "enum": ["InProgress", "Success", "AlreadyAtTarget", "NotStaged", "OperationFailed", "TargetBootFailed", "AgentInternalError", "InvalidRequest"] },
    "message":       { "type": "string", "maxLength": 2048 },
    "fromVersion":   { "type": "string" },
    "toVersion":     { "type": "string" },
    "startedUtc":    { "type": "string", "format": "date-time" },
    "lastUpdatedUtc": { "type": "string", "format": "date-time", "description": "When the agent last wrote this status. The agent refreshes it on every write, including a periodic InProgress heartbeat, so AKS-RP and the watchdog can tell a working agent from a stuck one." },
    "finishedUtc":   { "type": "string", "format": "date-time" }
  },
  "allOf": [
    {
      "if":   { "properties": { "code": { "const": "InProgress" } }, "required": ["code"] },
      "then": { "required": ["startedUtc", "lastUpdatedUtc"] },
      "else": { "required": ["startedUtc", "finishedUtc"] }
    }
  ]
}
```

One schema validates both status keys. `acl.azure.com/update-status` carries the status of
the requested operation. `acl.azure.com/update-commit-status` carries the post-reboot
`commit` status. Both keys use a bare UUID `operationId`, and the `commit` status repeats
the `operationId` of the `finalize` or `rollback` that caused the reboot.

### 2.2 Trident ACL Agent

The on-node half of the trigger is the Trident ACL agent, a binary baked into the ACL image
at `/usr/bin/trident-acl-agent`.
For the trigger mechanism to work, the agent needs to read the request annotation from its
own Node, invoke Trident, and write the status annotation back, including post-reboot.
The post-reboot status PATCH is what delivers the `commit`-success signal AKS-RP waits for
(see 2.3).

The execution model is open; either of the following satisfies this contract:

- **Short-lived (oneshot on boot + periodic systemd timer)**:
  Each run reads the current request annotation, decides what to do, calls Trident,
  writes back the status annotation, and exits.
  Trigger latency is bounded by the timer interval.
- **Long-lived daemon** started by systemd on boot that holds an API server watch on its
  own Node object, reads the request annotation from each event, reacts immediately on
  change, and writes the status annotation back from the same process.
  There is lower trigger latency and no polling cost on the API server, at the cost of a
  persistent resident process competing with customer workload for CPU and memory.

Both models run on boot, but neither one guarantees that a run *follows* a boot. The daemon
restarts after a crash and the short-lived model also fires on its timer, so in both cases
the agent can start again inside the same boot. The agent must therefore establish that a
boot has happened since it recorded the pending commit, and must not infer it from the fact
that it is running.

While an operation runs, the agent re-writes the `InProgress` status with a
fresh `lastUpdatedUtc` on a fixed interval. The interval must be well below the watchdog
SLA in 3, so that several heartbeats fall inside one SLA window. This gives AKS-RP and the
watchdog liveness so that a long `stage` on a slow link and an agent that died mid-operation
look different.

The agent refreshes `lastUpdatedUtc` on every status write, terminal writes included. On a
re-served cached status the agent writes the timestamp of the re-serve, so the field always
reports when the status was last written, not when the work happened. `startedUtc` and
`finishedUtc` carry the timing of the work itself.

The agent talks to the K8s API server using kubelet's persisted kubeconfig at
`/var/lib/kubelet/kubeconfig`.
The credential there is kubelet's `system:node:<nodeName>` identity in the
`system:nodes` group, written by kubelet's TLS bootstrap.

This pattern (annotation as trigger, kubelet's kubeconfig as the credential) is what
AKS's OS-patch system uses today.
[`mariner-package-update.sh`](https://github.com/Azure/AgentBaker/blob/66f450f66aaa48b03cdd2f948f4b89f003772ac5/parts/linux/cloud-init/artifacts/mariner/mariner-package-update.sh)
reads `kubernetes.azure.com/live-patching-golden-timestamp` from its Node, applies
the patch, and writes back `kubernetes.azure.com/live-patching-current-timestamp`,
using only `/var/lib/kubelet/kubeconfig` (no `ServiceAccount`, no token, no extra RBAC).

What that identity is actually allowed to do at the API server is set by two upstream
K8s layers.
The [Node authorizer](https://kubernetes.io/docs/reference/access-authn-authz/node/)
lets that identity write to Node objects (annotations included), but doesn't restrict
it to any specific Node.
This means that in principle, the kubelet could PATCH any Node in the cluster.
The [`NodeRestriction` admission
plugin](https://kubernetes.io/docs/reference/access-authn-authz/admission-controllers/#noderestriction)
adds that restriction.
When enabled, it rejects any kubelet write that targets a Node other than its own.

The agent only PATCHes its own Node, so the practical blast radius is one Node's
annotations and status.
If NodeRestriction is enabled, the API server enforces that same one-Node boundary
itself, so a buggy or compromised agent that tried to write elsewhere would be blocked
at the cluster.

NodeRestriction also caps what the agent may change on its *own* Node: it rejects any
kubelet-identity update that modifies `spec.taints`
([`noderestriction/admission.go`](https://github.com/kubernetes/kubernetes/blob/master/plugin/pkg/admission/noderestriction/admission.go)).
The agent therefore never tries to taint a node. Keeping a failed node out of rotation
relies instead on the cordon AKS-RP already applied before `finalize`, which survives the
reboot (2.5); a taint, if AKS-RP wants one, has to come from AKS-RP.

If a status PATCH fails because the Node object does not exist, the agent stops and does
not retry. AKS-RP deletes the Node when it gives up on the in-place update and replaces
the disk, so a `NotFound` means that the update is abandoned. The Node is the agent's only
channel, so there is nothing left to report to. AKS-RP stops its own wait on the same
signal, rather than waiting out the timeout for a status that cannot arrive.

### 2.3 Pre / post-reboot split and state persistence

A `finalize` (or `rollback`) reboots the node between two halves of the agent's work:
the pre-reboot run executes on one partition set and runs `finalize` against Trident,
the post-reboot run executes on the other partition set and runs `commit` against
Trident.

To satisfy the requirement that the `finalize` terminal status is written *before* the
reboot, the agent invokes Trident's gRPC `UpdateFinalize` with
`RebootManagement = CALLER_HANDLES_REBOOT` (per Trident's
[gRPC reboot management](https://microsoft.github.io/trident/docs/Explanation/gRPC-Server/#reboot-management)),
so Trident arms the boot configuration and returns a `Completed` response indicating a
reboot is required, without rebooting. The agent writes the terminal `finalize` status,
and only then triggers the reboot as a separate step. If arm and reboot happened in one
call (Trident's default `TRIDENT_HANDLES_REBOOT`), the reboot would kill the agent before
the status PATCH could land, and AKS-RP could not tell a successful finalize apart from a
crashed agent.

`rollback` follows the same pattern. `RollbackStage` stages the swap, the agent writes the
terminal `rollback` status, then `RollbackFinalize` runs with
`RebootManagement = CALLER_HANDLES_REBOOT` and the agent triggers the reboot.
`ManualRollbackKind = AB_ROLLBACK_REQUESTED` limits the operation to the A/B update, which
is what `--ab` does on the CLI.

The agent bridges the two halves through one file, `/var/lib/trident-acl-agent/state.json`:

```json
{
  "pendingCommit": {
    "nodeUpdateId":  "550e8400-e29b-41d4-a716-446655440000",
    "operationId":   "f47ac10b-58cc-4372-a567-0e02b2c3d479",
    "operation":     "finalize",
    "targetVersion": "202606.29.0",
    "fromVersion":   "202605.15.0",
    "startedUtc":    "2026-06-04T12:00:00Z",
    "bootMarker":    "cf3d79a2-24bc-431d-94d7-9c1ac4dfcdd1"
  },
  "completed": {
    "f47ac10b-58cc-4372-a567-0e02b2c3d479": {
      "operation": { /* finalize terminal status payload */ },
      "commit":    { /* commit terminal status payload */ }
    }
  }
}
```

The file has two parts.

**`pendingCommit`** is what gets handed off across the reboot.
The pre-reboot run writes it; the post-reboot run reads it to know what to commit, and which
`nodeUpdateId` / `operationId` to emit the `commit` status under.

The record asserts that an update is armed and a reboot is owed, so it is written only after
`UpdateFinalize` (or `RollbackFinalize`) returns success, and before the terminal status
PATCH. If it's written earlier, it can claim an armed boot that doesn't exist, and a run reading
it in that window commits a node with nothing staged, which Trident reports as a success.
If written later, a run in the gap finds no record and falls to the degraded path below.

`bootMarker` is how the post-reboot run tells a real reboot from a restart of the agent
inside the same boot. The agent records it with `pendingCommit` and compares it on every
later start: a different value means a boot happened, so it is safe to run `commit`, which
then reports whether the node landed on the target or the firmware fell back. The same value
means the reboot never happened and the armed `finalize` is still waiting for it. Any value
that is opaque, stable within a boot, and different across boots will do, so where it comes
from is left open (3).

**`completed`** is the agent's record of work it has already finished, so a repeated request
gets the same answer without the work being redone.
A status watch can time out after the PATCH already landed, a controller can re-reconcile, and
the oneshot agent can run again on a later timer tick, so the same operation can come around
more than once. The map holds one entry per finished `operationId`, and each entry holds the
exact terminal status payloads the agent emitted (the status annotation payloads from 2.1);
on a repeat, the agent re-writes the matching payload instead of driving Trident again.

An entry has one member per phase, mirroring the two status keys in 2.1:

- `operation` — the terminal status of the requested operation, re-served to
  `acl.azure.com/update-status`. It is re-hit when **AKS-RP re-PATCHes** the same request,
  e.g. after it missed the status watch, and on every later agent run, because the request
  annotation stays on the Node after the operation completes.
- `commit` — the terminal status of the post-reboot `commit`, re-served to
  `acl.azure.com/update-commit-status`. It is re-hit by the **agent's own post-reboot
  re-runs**, since the oneshot can fire several times before AKS-RP sees the terminal.

Both members are optional. A `stage` never gets a `commit`, since it does not reboot the
node. An entry rebuilt on the degraded path below can carry a `commit` with no `operation`.

Keying the entry by `operationId` alone, with the phase inside it, keeps this file in step
with the contract: the `commit` is not a separate operation, it is the second half of one.
A re-serve writes each phase to that phase's own annotation key, so a repeated read of a
completed `finalize` request cannot disturb the `commit` status.

**Surviving the partition swap**

All of the above assumes `state.json` is still there after
the reboot, so it has to sit on a mount Trident carries across when it swaps partition sets.
If it doesn't, the post-reboot agent starts from an empty file: it sees an unfinished
`finalize` in the annotation but can't tell whether the reboot already happened or AKS-RP just
PATCHed a fresh `finalize` it has yet to run.

In that degraded case there is no `pendingCommit`, and so no `bootMarker` either, so the
agent reconstructs the answer from the node's active version (queried from Trident) and
Trident's boot history — the annotation only carries `targetVersion`, not the previous one.
The version on its own is not enough: a node that has not rebooted yet and a node whose
target failed to boot are both sitting on the previous version and look identical. Only the
boot history separates them.

- **active == `targetVersion`** — the swap happened (the active version only changes via the
  post-`finalize` swap), so the agent runs `commit`.
- **active != `targetVersion`, and `get rollback-chain` / `get last-error` show the target
  was armed and failed to boot** — the firmware fell back, so the agent emits
  `TargetBootFailed` (node up on its previous version) rather than forcing a
  reprovision.
- **active != `targetVersion`, and Trident shows no boot was attempted** — the reboot hasn't
  happened yet, so this is a fresh `finalize`; the agent runs it normally.
- **anything Trident can't account for** — the agent emits `AgentInternalError` and lets the
  watchdog reprovision the node.

`commit` cannot serve as the probe that answers this. Called before the reboot it refuses to
promote a volume the node never booted, but it also resets the host to `Provisioned`,
discards the armed update, and reports the refusal with the same error an actual failed
boot produces. The question has to be answered before `commit` is called.

Either way, the real fix is getting `state.json` onto a swap-surviving mount.

### 2.4 Watchdog

The annotation mechanism is best-effort on the return path.
If the node never rejoins the cluster, if the agent's kubeconfig did not survive the swap,
or if the agent crashes on the new partition, the `InProgress` annotation persists with no
on-node process able to clear it.
AKS will therefore run an out-of-band watchdog that detects a stranded `InProgress` past
the SLA and reprovisions the node.

The watchdog measures staleness from `lastUpdatedUtc`, not from `startedUtc`. An operation
that still writes heartbeats is alive and gets the full time it needs. An `InProgress`
whose `lastUpdatedUtc` stops advancing is a stranded status, whatever its `startedUtc`
says. This distinction is what keeps a slow but healthy `stage` from being reprovisioned.

This is not specific to annotations: any A/B update mechanism on AKS that involves a reboot
needs the same fallback, since there is no way to authoritatively confirm a successful
reboot from outside the node.

On top of the reboot itself being best-effort, the annotation transport adds a second
best-effort step: even when the node comes back and kubelet is `Ready`, the post-reboot
agent still has to write the terminal status PATCH, and any number of things can stop
that (kubeconfig did not survive the swap, agent crash, API server unreachable from the
node at that moment).
Because of this, the watchdog has to cover both "node never came back" and "node came back
but the status PATCH never landed".

### 2.5 Rollback and failure recovery

AKS-RP owns every rollback decision. The node never rolls back on its own: both failure
modes below are surfaced to AKS-RP through the status annotation, and AKS-RP decides and
initiates whatever recovery follows. Trident's own
[health-check-driven rollback](https://microsoft.github.io/trident/docs/Explanation/Health-Checks/)
is therefore left off, by declaring no `health.checks` in the Host Configuration (see 3).
That matters concretely: health checks run *inside* `trident commit`, and on an A/B update a
failing check makes `commit` itself roll the node back and reboot it into the previous OS.

The two failure modes reach AKS-RP differently:

- **Boot failure.** The `finalize` phase of `trident update` sets the one-shot UEFI
  `BootNext` to the target OS and leaves `BootOrder` booting the previous (servicing) OS,
  with the target placed last; only a successful `commit` moves the target's boot entry to
  the front of `BootOrder`
  ([UEFI variable management](https://microsoft.github.io/trident/docs/Explanation/UEFI-Variables/)).
  A target that fails to boot therefore never commits: the firmware falls through to the
  unchanged `BootOrder` and the node comes back on its prior, still-current version rather
  than being lost. Nothing was promoted, so the update simply never took effect. Trident's
  docs call this fall-through a *rollback*, which is a wording difference worth noting: it is
  a firmware-level fallback, not a decision anything on the node made. The status code is
  named `TargetBootFailed` for that reason, to keep it distinct from the `rollback` AKS-RP
  requests. The post-reboot agent detects it (`state.json` shows a pending commit whose
  target is not the running version; `trident get rollback-chain` / `get last-error`
  confirm the failed boot) and surfaces `TargetBootFailed`.
  The node does not put itself back into service. AKS-RP cordoned and drained it before
  `finalize` (2.6), and `spec.unschedulable` lives on the Node object rather than on disk, so
  it survives the reboot and the node comes back still cordoned. The fall-through is inherent
  to Trident's A/B mechanic and cannot be suppressed without leaving an unbootable node down
  (see 3), and it still honors AKS-RP ownership: the node returns to service only on AKS-RP's
  command.
- **Booted but unhealthy.** The node boots the new version and `commit` succeeds, but
  AKS-RP's telemetry judges it unhealthy. With Trident health checks off, the OS does not
  act — AKS-RP initiates the rollback itself.

The watchdog (2.4) remains a backstop for the rarer case where the node is stuck and no
terminal status is ever written.

The status annotation is a telemetry channel: the agent reports what Trident did, and
AKS-RP reacts from its own signals. The two `commit` outcomes are handled differently.

On **`Success`**, the node is up on the new version and AKS-RP judges its health, then picks
one of:

- **Accept** — the node is healthy on the new version.
- **Partition-swap `rollback`** — if AKS-RP decides the update failed while the node is
  still reachable, it PATCHes a `rollback` request for a fast swap back to the previous
  partition and waits for that operation's `commit` terminal. Executed via
  [`trident rollback --ab`](https://microsoft.github.io/trident/docs/Explanation/Manual-Rollback/).
  The `--ab` matters: a bare `trident rollback` undoes the last update of *either* kind and
  would pick a runtime update if one had been applied more recently. This path can only undo
  the most recent A/B update, so it cannot walk a node back more than one version. 2.3 covers
  how the agent preserves status-before-reboot ordering.
- **Re-image** — if the node is unreachable (or for a user-initiated rollback), re-image to
  a tracked gallery image instead, since partition state is not tracked per node.

On **`TargetBootFailed`**, the forward update never committed and the node is still on its
prior version, still cordoned from the pre-`finalize` drain (see the boot-failure bullet
above). There is nothing to swap back, so AKS-RP either uncordons the node on its previous
version and retries the update later, or re-images it to put it on a specific version.

### 2.6 AKS integration

AKS rolls out agent-pool changes via `UpgradeVmssPool.UpgradePool()` in
`vmsspoolupgrader.go`, which selects a per-pool "upgrader" based on pool type.
Normal pools use `vmssInstancesUpgrader`, with `vmssSpotInstancesUpgrader`,
`vmssGatewayInstancesUpgrader`, and `vmssInstancesBlueGreenUpgrader` as variants.
ACL pools would get a fifth branch, `vmssACLInstancesUpgrader`.

The customer picks one of two options for rolling updates:
`MaxSurge` (provision N extra nodes upfront, then upgrade originals) or `MaxUnavailable`
(take up to N originals out of rotation at a time, no extra capacity).

`vmssInstancesUpgrader` (the standard upgrader) processes nodes in parallel up to the
configured `MaxSurge` / `MaxUnavailable` budget.
Its per-node body, `upgradeSingleNode()`, cordons and drains the VM, waits for disk
detach, calls CRP `manualupgrade` to swap the OS disk, deletes the Node, calls CRP
`reimage`, and waits for kubelet to re-register.

`vmssACLInstancesUpgrader.upgradeSingleNode()` issues `stage` first, while the node is still
serving workloads (staging before drain shrinks the node-unavailable window to just reboot
and commit).
It then cordons, drains, and soaks, issues `finalize`, and waits for the `commit` terminal
status.
The disk-detach, CRP `manualupgrade`, Node delete, and CRP `reimage` steps are all dropped
because the partition swap happens on the existing OS disk, so the Node object stays put and
the annotation stays where the post-reboot agent can read it.

Post-`commit`, AKS-RP judges the node's real health from its own telemetry (did it rejoin
the cluster, are workloads scheduling), not from an OS-level signal — Trident's health
checks are left off (see 3). From there it accepts the node, PATCHes a partition-swap
`rollback`, or re-images, per 2.5. A `TargetBootFailed` commit means the target never
booted, so the update never took effect and the node is back on its previous version and
still cordoned; AKS-RP owns the next action there too (2.5).
On any watchdog timeout, AKS-RP falls back to the standard delete-Node-and-reprovision
path. Every await below has one, so the diagram shows this as a single edge instead of one
per await.

A `stage` that fails takes the same fallback. The node is untouched and still serving at
that point, because `stage` runs before the cordon and writes only to the inactive
partition. AKS-RP can therefore retry the `stage` with a new `operationId`, or give up on
the in-place update and replace the disk. The second path deletes the Node object while
the agent may still be running, which 2.2 covers.

```mermaid
flowchart TD
    Stage["PATCH stage<br/>await update-status"] -- Success / AlreadyAtTarget --> Pre["cordon, drain, soak"]
    Stage -- failure --> NodeDel["delete Node + reprovision"]
    Pre --> Finalize["PATCH finalize<br/>await update-status"]
    Finalize -- Success --> Commit["node reboots<br/>await update-commit-status"]
    Commit -- Success --> Health{"AKS-RP telemetry:<br/>node healthy?"}
    Commit -- TargetBootFailed --> Rolled["node up on old OS<br/>(AKS-RP returns it to service)"]
    Health -- yes --> Done["node Ready on new OS"]
    Health -- "no, reachable" --> Rollback["PATCH rollback<br/>(partition swap, same two steps)"]
    Health -- "no, unreachable / user-initiated" --> Reimage["re-image to gallery image"]
    Rollback --> Rolled
    Await["any await past its SLA"] --> NodeDel
```

One tweak to this process is to lift `stage` out of `upgradeSingleNode` into a pool-wide
phase that runs on every node before any per-node drain/finalize starts.
Upfront staging is faster and fails fast: a broken image is caught across the whole pool
before the first node is taken out of rotation.
The trade-offs are concurrent image pulls on every node at once and a structural change to
the standard upgrader instead of an isolated per-pool branch.
MVP is per-node for now (see 3).

### 2.7 Metrics

For health, count each terminal `code` per operation and alert on non-`Success` spikes.
A counter labelled by `operation` and `code` allows slicing by failure mode.
For example, a spike in `TargetBootFailed` points at a bad image, a spike in
`OperationFailed` at staging or image-delivery trouble, and a spike in
`AgentInternalError` at the agent itself.
Watchdog firings would catch cases where no terminal code gets written at all.

For performance, track per-node downtime and per-node rollback duration.
Downtime is defined as the window between cordon and the Node being back to `Ready` on
the new partition.
Rollback duration is the same idea on the failure path: from a `rollback` request (or a
`TargetBootFailed` boot-failure return) to the node being back up on the previous
partition.
Both of these measurements will be useful for comparison against non-A/B updates, and for
any SLAs.
(`stage` duration is worth tracking on its own as well, since a long tail will make the
case for moving to the pool-wide upfront variant in 2.6).

## 3. Decisions and Open Questions

### Decided

| Owner | Topic | Decision |
|---|---|---|
| PMC + AKS | Trigger mechanism. | Annotations. A pod-based cluster extension and a manifest-only VM extension were both considered and rejected (see the introduction). |
| AKS | Rollback model. | AKS-RP owns every rollback decision and the node never rolls back on its own. Both boot failure and post-boot health failure are surfaced to AKS-RP, which decides from its own telemetry rather than an OS-level signal. |
| AKS + Polar | Trident health checks. | Left off, so there is no OS-level auto-rollback. AKS-RP's telemetry owns the health decision and initiates any rollback (2.5). |
| AKS + Polar | Boot-failure recovery ownership. | Accepted as-is: a target that fails to boot falls through to the previous OS and the node comes back still cordoned, for AKS-RP to act on (2.5).|
| PMC + AKS | Concurrency during an in-flight operation. | A new `operationId` that arrives while a `finalize` is in flight is rejected with `InvalidRequest`, and AKS-RP waits for the in-flight operation's terminal status (2.1). |
| AKS | `stage` scheduling. | Per-node for MVP. A pool-wide `stage` ahead of any drain stays a later optimization, since it changes AKS-RP's scheduling rather than the contract (2.6). |

### Open

| Owner | Question | Proposed / status |
|---|---|---|
| AKS + Polar | Boot-then-panic: the node boots the target, consuming the one-shot `BootNext`, then crashes before `commit`. | The node is not stranded on the target: `finalize` leaves `BootOrder` booting the servicing OS with the target last, and only `commit` moves the target to the front ([UEFI variable management](https://microsoft.github.io/trident/docs/Explanation/UEFI-Variables/)), so any later boot returns to the previous OS. Open: whether the ACL image reboots itself after a panic (kernel `panic=<seconds>`, or an equivalent watchdog), since a node that halts never takes that path. The watchdog (2.4) is the outer backstop. |
| Polar + AKS | Is the version a running ACL node reports comparable to the `targetVersion` AKS-RP writes (sourced from `linux_sig_version.json`, `YYYYMM.DD.PATCH`)? `AlreadyAtTarget`, `fromVersion`, and 2.3's degraded-path reconstruction all compare the two, and Omaha requires the node to send its installed version. | Currently, it looks like the release version is not exposed as an OS property — `os-release` ships on the verity-protected `/usr` volume and carries only `VERSION_ID=3.0.20260616` and `BUILD_ID=1140235`, with no `IMAGE_VERSION` field. Open: needs the release version carried on the volume it describes; setting `IMAGE_VERSION` in `os-release` at build time would satisfy this. |
| AKS | Watchdog SLA, and the agent's heartbeat interval for `lastUpdatedUtc` (2.2). | Proposed: ~10 min of `lastUpdatedUtc` staleness on an `InProgress` status. The heartbeat interval has to be well under that; the two are set together. |
| Polar | Canonical source for node name on an ACL OS box (`/etc/kubernetes/azure.json`, kubelet's `--hostname-override`, lowercased `hostname`, something else). | Proposed: lowercased `hostname`, which is what `mariner-package-update.sh` uses on AKS Mariner today. |
| Polar | Where the agent's `bootMarker` (2.3) comes from: a value the agent holds itself, or one it reads back from Trident. | Not blocking. The agent can use the kernel's `/proc/sys/kernel/random/boot_id`, or a marker file on a `tmpfs` mount such as `/run`, with no dependency on anyone. Trident holds the same answer in the `BootNext` variable it arms at finalize and never clears, but does not expose it over gRPC. Reading it from Trident would drop the field from `state.json` entirely. |

## 4. Dependencies

| Owner | Dependency | Impact | Status |
|---|---|---|---|
| Polar | Kubelet's kubeconfig present on the ACL image at `/var/lib/kubelet/kubeconfig` (or equivalent). | Gates the agent reaching the API server at all. Same pattern is used today by `mariner-package-update.sh` and `localdns.sh` on AKS Mariner / Azure Linux (see 2.2). | Closed. `/var` is on the shared root, which A/B does not swap. |
| Polar | Post-reboot, kubelet's kubeconfig is reachable by the agent. | Gates the post-reboot return path working at all. | Closed. Only `/usr` swaps, so `/var/lib/kubelet/` carries across. |
| Polar | Swap-surviving mount picked for `/var/lib/trident-acl-agent/state.json`. | Gates the post-reboot `commit` half of `finalize` / `rollback` working. | Closed. `/var` survives the swap. No separate mount is needed. |
| Polar | TAA binary baked into the ACL image at `/usr/bin/trident-acl-agent`, with systemd units for the chosen execution model. | Gates MVP. | Open. Implementation in progress against this contract. |
| Polar | `tridentd` present and socket-activated on the ACL image, serving the gRPC API at `/run/trident/trident.sock` (owned `root:root`, mode `0600`), with the agent running as root so it can connect. | Gates the caller-handled-reboot ordering in 2.3. Without the daemon the agent falls back to the CLI, whose `finalize` reboots before a terminal status can be written. | Closed. |
| AKS | New ACL-pool branch `vmssACLInstancesUpgrader`, selected in `UpgradeVmssPool.UpgradePool()` for ACL pools. See 2.6. | Gates end-to-end integration. | Open. PMC may contribute the patch. |
| AKS | Watchdog implementation. | Backstop for silent failures on the return path. Without it, an `InProgress` annotation that never resolves can leave the node stuck. | Open. |

## 5. Timeline

| Phase | Target | Scope |
|---|---|---|
| API design | mid-June | Annotation keys, payload, operations, result codes finalized. Confirm with Polar. |
| High level design | end of June | Working PoC. Open questions resolved and dependencies are acknowledged/unblocked. |
| MVP | end of July | One AKS-triggered update end-to-end on a dev cluster node, validating AKS node-pool integration. Would require production-ready agent baked into the ACL image. |
| Integration | end of August | Multi-node ACL update on a dev cluster driven by AKS's upgrade workflow + watchdog. Confirm that bootstrapping + rest of AKS-RP workflow works post-swap. |
| Hardening and prod readiness | end of September | Fix any issues from integration. Set up metrics for service health and performance measurement. |
