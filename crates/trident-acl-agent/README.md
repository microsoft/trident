# trident-acl-agent

The on-node half of Trident's Azure Container Linux (ACL) A/B update
trigger. Runs in one of two modes, selected by
`TRIDENT_ACL_AGENT_ORCHESTRATION_GOAL_SOURCE`:

- **`annotations`** (the default): watches its Node's
  `acl.azure.com/update-request` annotation and drives Trident's
  stage/finalize/rollback/commit operations against `tridentd` accordingly,
  reporting progress and status back to Kubernetes and to Nebraska (the
  Omaha-protocol update server).
- **`omaha-only`**: the historical one-shot behavior. Queries Nebraska once,
  and if an update is offered, calls tridentd's combined `update()` RPC once
  and exits - no Kubernetes or annotation involvement at all. Kept as an
  explicit opt-out for nodes that don't participate in the AKS
  annotation-driven update protocol.

## Configuration

There is no config file. Every setting is an environment variable prefixed
`TRIDENT_ACL_AGENT_`, systemd-style: set it directly in the unit's own
`Environment=` lines, via a drop-in override (`systemctl edit
trident-acl-agent.service`, which creates
`/etc/systemd/system/trident-acl-agent.service.d/override.conf`), or by any
other means that ultimately sets the process's environment before it
starts.

A variable that is unset, or set to the empty string, falls back to that
setting's default below. A variable that is set to a malformed value (a bad
URL, a bad duration, an unrecognized `goal_source`) causes the agent to
fail to start with an error naming the offending variable.

| Variable | Default | Description |
|---|---|---|
| `TRIDENT_ACL_AGENT_NEBRASKA_ENDPOINT` | `https://nebraska.example.invalid/v1/update` (deliberately unreachable) | The Nebraska/Omaha server URL to poll for updates and report progress/completion events to. Can also be overridden per-update via the `server` field on the `acl.azure.com/update-request` annotation, which takes precedence over this variable for that update's entire lifecycle (stage through post-reboot commit). |
| `TRIDENT_ACL_AGENT_NEBRASKA_APP_ID` | An all-zero UUID (deliberately invalid) | The Nebraska application ID this node checks in as. Can also be overridden per-update via the `appId` annotation field, same precedence rules as the endpoint. |
| `TRIDENT_ACL_AGENT_NEBRASKA_TRACK` | `unspecified` (deliberately invalid) | The Nebraska track (channel/group) this node follows. Can also be overridden per-update via the `track` annotation field, same precedence rules as the endpoint. |
| `TRIDENT_ACL_AGENT_KUBERNETES_API_SERVER` | unset | Explicit override for the Kubernetes API server URL. When unset, the server embedded in `TRIDENT_ACL_AGENT_KUBERNETES_KUBECONFIG`'s own kubeconfig is used as-is (e.g. the real cluster FQDN a node's own `/var/lib/kubelet/kubeconfig` already points at). Only needed when the kubeconfig's own server is wrong for this deployment. |
| `TRIDENT_ACL_AGENT_KUBERNETES_KUBECONFIG` | `/var/lib/kubelet/kubeconfig` | Path to the kubeconfig file used to reach the Kubernetes API server and authenticate as this node. |
| `TRIDENT_ACL_AGENT_KUBERNETES_NODE_NAME` | The node's own hostname, lowercased | The Node object this agent watches/patches. Kubernetes Node names must be valid RFC 1123 DNS labels (lowercase), matching how kubelet itself registers the Node - so the default only needs overriding when the agent's environment can't discover the correct hostname on its own. |
| `TRIDENT_ACL_AGENT_TRIDENT_SOCKET` | `unix:///run/trident/trident.sock` | The gRPC Unix socket URI used to reach `tridentd`. |
| `TRIDENT_ACL_AGENT_ORCHESTRATION_GOAL_SOURCE` | `annotations` | Selects the agent's operating mode: `annotations` or `omaha-only` (see above). |
| `TRIDENT_ACL_AGENT_ORCHESTRATION_STATE_PATH` | `/var/lib/trident-acl-agent/state.json` | Path to the agent's persistent state file, which bridges the pre-reboot `finalize`/`rollback` half of an update and its post-reboot `commit` half across the reboot. |
| `TRIDENT_ACL_AGENT_ORCHESTRATION_STAGE_TIMEOUT` | `20m` | How long a `stage` operation (parsed as a [`humantime`](https://docs.rs/humantime) duration, e.g. `20m`, `1h`) is allowed to run before it's considered failed. |
| `TRIDENT_ACL_AGENT_ORCHESTRATION_FINALIZE_TIMEOUT` | `10m` | How long a `finalize` operation is allowed to run before it's considered failed. Parsed the same way as the stage timeout. |
| `TRIDENT_ACL_AGENT_ORCHESTRATION_HEARTBEAT_INTERVAL` | `60s` | Refresh cadence for the `InProgress` status heartbeat the agent writes while a stage/finalize/rollback operation is running, so AKS-RP and the watchdog can tell a working agent from a stuck one. Parsed the same way as the timeouts. |

## Diagnostics

`trident-acl-agent --validate-connection <kubernetes|tridentd|nebraska>`
checks connectivity to a single dependency using the current environment
and exits immediately - useful for a systemd `ExecStartPre` check or manual
on-node troubleshooting without running the full orchestrator loop.
