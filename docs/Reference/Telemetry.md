---
sidebar_position: 4
---

# Telemetry

Trident records the same metrics/spans locally in two places regardless of
whether remote telemetry is enabled: appended to
`/var/log/trident-metrics.jsonl`, and logged to journald under the
`trident-tracing` syslog identifier. Retrieve the journald copy with:

``` bash
journalctl -t trident-tracing
```

On top of these local copies, Trident can optionally send this same
best-effort stream of tracing data to Azure Monitor / Application
Insights. See [Agent Configuration](./Agent-Configuration.md) for how to
enable it.

## Host Metadata

Every event sent also includes as much of the following host metadata as
is available at the time, so operators should be aware this leaves the
host along with the metrics/spans themselves:

- `asset_id`: the host's DMI product UUID (a stable hardware identifier).
- `os_release`: the `VERSION` field from `/etc/os-release`.
- `kernel_version`: the running kernel release (`uname -r`).
- `total_cpu`: the number of CPUs.
- `total_memory_gib`: total memory, in GiB.
- `trident_version`: the running Trident version.
- `database_id`: an ID that lets separate events be correlated back to the
  same datastore over its entire lifetime (generated on first access to
  the datastore, whether or not an install has actually happened yet).
  Not present on events that fire before the datastore has ever been
  accessed (e.g. very early in a host's first-ever `install`, before
  `Trident::new` opens or creates it).
- `installation_id`: an ID that lets separate events be correlated back to
  the same host installation over time. Unlike `database_id`, this is
  only ever created (get-or-create, never overwritten) at the start of
  `Trident::install`. Any event that fires before that point (i.e.
  before a host's first-ever install has actually created one) instead
  reports that invocation's own `operation_id` as a stand-in
  `installation_id` -- the same value that will end up persisted as the
  real `installation_id` if that invocation goes on to become the
  first-ever install. A datastore created before `installation_id` was
  introduced is migrated the first time it is opened: a one-time write
  persists an `installation_id` for it (without touching any other
  data), so older hosts pick up the field on their next command rather
  than remaining permanently without one.
- `operation_id`: an ID that lets events emitted during the same command
  invocation be correlated with each other.
- `command`: which command produced the event (e.g. `install`, `update`,
  `update_stage`, `update_finalize`, `commit`, `rollback`, `rebuild_raid`).
- `source`: which of Trident's three entry points produced the event --
  `cli` (a command run directly, without a daemon), `daemon` (a command
  the daemon executed for a gRPC request), or `grpc-client` (the CLI
  acting as a client, relaying a command to a running daemon).

## Command Errors

If a *servicing* command (`install`, `update`, `commit`, `rollback`,
`rebuild_raid`, and their gRPC/`grpc-client` equivalents) fails, a
`command_error` event is also sent (tagged with the same
`operation_id`/`command` as above), breaking the failure down into:

- `kind`: the top-level error category (e.g. `internal`, `invalid-input`,
  `servicing`, `initialization`).
- `subkind`: the specific error within that category (e.g.
  `check-root-privileges`), when one applies.
- `location`: the `file:line` in Trident's source where the error was
  originally raised.

A `grpc-client` invocation only fires its own `command_error` when the
daemon it talked to never actually responded (a transport-level failure:
the daemon's socket wasn't found, the connection was refused, or it
dropped mid-call). If the daemon did respond -- including rejecting the
request outright -- the daemon's own `command_error` for that failure
already has full `kind`/`subkind`/`location` fidelity, so `grpc-client`
stays silent rather than reporting the same failure again under a
generic classification.

## Delivery

Telemetry delivery is always best-effort and never affects servicing
outcomes, but failures are not all logged at the same level: a failure to
serialize an event, or to enqueue it because the background uploader has
already shut down, is logged at trace level, while a failure to actually
deliver an event (e.g. no network connectivity, or a non-2xx response from
Application Insights) is logged at error level, so operators can find
remote-delivery problems in normal logs.
