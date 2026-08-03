# Trident ACL Agent: Label Protocol Design Decisions & Open Questions

This document tracks the design decisions and open questions behind the
label-driven A/B update protocol implemented in this crate (see
`state-engine.md` for how the implementation actually behaves). It exists so
this crate is self-contained — no external wiki reference is required to
understand why the code makes the choices it does.

## Decisions

| # | Decision | Status | Notes |
|---|---|---|---|
| 1 | Reboot ownership during finalize | Decided | `CallerHandlesReboot` — the **agent** issues the actual reboot, not `tridentd` — so the agent can attempt the `state=finalized` label patch before the machine goes down. |
| 2 | Label patch failure handling | Decided | Best-effort: bounded retry, then proceed (reboot / continue) regardless. Trident's local state is the source of truth; labels are a status mirror, never a blocker for the servicing operation itself. |
| 3 | Omaha/Nebraska stays the content channel | Decided | Labels never carry the artifact; `target-os-image-version` is a safety gate against what Nebraska currently offers, not a delivery mechanism. |
| 3a | Agent crafts `HostConfiguration`, doesn't download the image | Decided | Nebraska returns an image URL (+ hash); the agent builds a minimal `HostConfiguration` pointer from it, reusing the existing `image:\n  url:...\n  sha384:...` template. `tridentd`'s own `update_stage` fetches/applies the actual image — no new download/hash-verification code is needed in the agent. |
| 4 | Post-reboot resume authority | Decided | On every startup, the agent checks `tridentd`'s own `ServicingState` (and, since the crash-recovery fix below, `GetLastError`) **before** trusting/reading any label — labels may be stale or lost across a reboot. See `state-engine.md`'s "Startup: recovering from `tridentd`" section. |
| 5 | Stuck-node fallback | Decided (shape) | Quarantine label → diagnostics capture → hand off to the existing reimage/replace path (owned by `aks-rp`, outside this crate). Exact liveness-recheck-before-reimage policy: open (see below). |
| 6 | Single vs. dual transport (Omaha polling loop vs. label-driven) | Decided (shape) | A transport-agnostic `(waker, goal-source)` pair, runtime-selected via `[orchestration].goal_source` in `/etc/trident/trident-acl-agent.conf` (see the config schema below). This label protocol is one such `goal-source`; Omaha polling for content discovery continues regardless of which orchestration transport is active. Endpoint overrides in the same file (Nebraska URL, K8s API server, Trident socket) are what let the agent run unmodified against test doubles — see `trident-acl-agent-tester` under `tools/cmd/`. |
| 7 | RBAC / NodeRestriction for agent self-labeling | **Open — needs security sign-off** | No existing precedent for a node-local process patching its own Node object. Kubelet's identity already sets `kubernetes.azure.com/*` labels once at bootstrap (`--node-labels`); whether a long-running process can/should reuse that identity for live PATCHes, or needs its own scoped credential, is unresolved. **As implemented**, `NodeClient::new`'s explicit-kubeconfig-path branch (`k8s.rs`) is identity-agnostic: it does not enforce which credential the configured kubeconfig contains, so nothing in this crate currently prevents pointing `[kubernetes].kubeconfig` at kubelet's own kubeconfig. This decision remains open; the config schema's example path should not be read as an endorsed production configuration. |

## Open questions (not yet decided)

- Exact RBAC shape for agent self-labeling (see decision #7) — this is the
  most consequential open item and should be resolved before production
  rollout.
- Liveness-recheck-before-reimage policy for the stuck-node fallback
  (decision #5).
- **Timeout specifics.** Per-phase timeouts have a *shape* (COSI-download-sized
  for stage, reboot+commit-sized for finalize) but not validated concrete
  numbers. The current `stage_timeout`/`finalize_timeout` defaults (`20m`/`10m`
  in `config.rs`) are placeholders pending real data from
  `trident-acl-agent-tester` scenario runs. Open: whether these should be
  static constants or configurable per-SKU/pool (large COSIs on slow storage
  vs. fast NVMe-backed SKUs may need different bounds).
- **What gates this functionality in `aks-rp`?** Not every node pool/cluster
  should attempt trident-acl-agent orchestration — needs an explicit
  eligibility check before `aks-rp` ever writes a `stage` label. Candidate
  gates: agent pool OS type (ACL only), VM SKU (does every SKU ACL supports
  actually support A/B update — e.g. ephemeral OS disk constraints,
  confidential VM/TrustedLaunch interactions), a feature-flag/allowlist for
  staged rollout, and/or a minimum trident-acl-agent version reported via
  label. This is entirely `aks-rp`-side and out of this crate's scope, but
  is a prerequisite for safely enabling the label protocol in production.
- **Activation mechanism if trident-acl-agent ships inactive.** If the agent
  binary is present in the image but disabled by default, what turns it on
  for a given node/pool? As implemented, activation is config-file-only
  (`[orchestration].goal_source = "labels"`, default `"omaha-only"`) — see
  decision #6 and `state-engine.md`'s assumption #2. Options considered but
  not adopted for v1: a VM/cluster extension flipping the config live, or
  using the first `stage` label itself as the activation signal (rejected as
  circular — it needs the agent's reconcile loop already running to observe
  the label). AgentBaker-injected config at bootstrap (decided) is the
  simplest activation path but means activation is fixed at node-creation
  time; retroactively activating/deactivating an already-running node
  without a reimage is not supported.

## Configuration file schema (endpoint overrides)

A single `/etc/trident/trident-acl-agent.conf` carries both the orchestration
transport choice (decision #6) and the concrete endpoints the agent talks to.
Making these overridable — not hardcoded — is what allows the same production
binary to run unmodified against test doubles instead of real
aks-rp/kubelet/Nebraska (see `tools/cmd/trident-acl-agent-tester`).

```toml
[nebraska]
endpoint = "https://nebraska.prod.example.com"
app_id = "..."
poll_interval = "5m"

[kubernetes]
api_server = "https://kubernetes.default.svc"
kubeconfig = "/var/lib/kubelet/kubeconfig"  # example only — see decision #7; not an endorsed production value
node_name = "$NODE_NAME"

[trident]
socket = "/run/trident/trident.sock"

[orchestration]
goal_source = "labels"   # or "omaha-only"
```

This file does not resolve the RBAC question (decision #7) — whatever
identity the agent authenticates as, the config file only controls *where*
it points, not *what it's allowed to do* there.
