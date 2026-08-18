# trident-acl-agent

One-shot Omaha/Nebraska update client for Trident's Azure Container Linux
(ACL) A/B update trigger. Queries Nebraska (the Omaha-protocol update
server) once, and if an update is offered, calls tridentd's combined
`update()` RPC once and exits - no Kubernetes or annotation involvement at
all. Intended for nodes that don't participate in the AKS annotation-driven
update protocol; invoked directly by an external scheduler (e.g. cron, a
timer unit) rather than running as a long-lived daemon.

Ships with **no systemd unit**. For the long-running, annotation-driven
sidecar with a systemd unit, see `trident-aks-agent`. Logic shared between
the two binaries (the Nebraska client, the tridentd gRPC client,
machine-id helpers) lives in `trident-agent-core`.

## Configuration

There is no config file. Every setting is an environment variable prefixed
`TRIDENT_ACL_AGENT_`, set however the invoking scheduler sets process
environment (e.g. a timer unit's `Environment=` lines, a drop-in override,
or a wrapper script).

A variable that is unset, or set to the empty string, falls back to that
setting's default below. A variable that is set to a malformed value (a bad
URL, a bad duration) causes the agent to fail to start with an error naming
the offending variable.

| Variable | Default | Description |
|---|---|---|
| `TRIDENT_ACL_AGENT_NEBRASKA_ENDPOINT` | `https://nebraska.example.invalid/v1/update` (deliberately unreachable) | The Nebraska/Omaha server URL to poll for updates and report progress/completion events to. |
| `TRIDENT_ACL_AGENT_NEBRASKA_APP_ID` | An all-zero UUID (deliberately invalid) | The Nebraska application ID this node checks in as. |
| `TRIDENT_ACL_AGENT_NEBRASKA_TRACK` | `unspecified` (deliberately invalid) | The Nebraska track (channel/group) this node follows. |
| `TRIDENT_ACL_AGENT_TRIDENT_SOCKET` | `unix:///run/trident/trident.sock` | The gRPC Unix socket URI used to reach `tridentd`. |
| `TRIDENT_ACL_AGENT_ORCHESTRATION_STAGE_TIMEOUT` | `20m` | How long the `stage` phase of the combined `update()` call (parsed as a [`humantime`](https://docs.rs/humantime) duration, e.g. `20m`, `1h`) is allowed to run before it's considered failed. |
| `TRIDENT_ACL_AGENT_ORCHESTRATION_FINALIZE_TIMEOUT` | `10m` | How long the `finalize` phase is allowed to run before it's considered failed. Parsed the same way as the stage timeout. |

## Diagnostics

`trident-acl-agent --validate-connection <tridentd|nebraska>` checks
connectivity to a single dependency using the current environment and
exits immediately - useful for manual on-node troubleshooting without
running a full update check.
