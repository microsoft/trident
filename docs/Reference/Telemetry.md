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

Every event sent also includes the following host metadata, so operators
should be aware this leaves the host along with the metrics/spans
themselves:

- `asset_id`: the host's DMI product UUID (a stable hardware identifier).
- `os_release`: the `VERSION` field from `/etc/os-release`.
- `kernel_version`: the running kernel release (`uname -r`).
- `total_cpu`: the number of CPUs.
- `total_memory_gib`: total memory, in GiB.
- `trident_version`: the running Trident version.
- `correlation_id`: an ID that lets separate events be correlated back to
  the same host installation over time.
- `operation_id`: an ID that lets events emitted during the same command
  invocation be correlated with each other.
- `command`: which command produced the event (e.g. `install`, `update`,
  `update_stage`, `update_finalize`, `commit`, `rollback`, `rebuild_raid`).

## Command Errors

If a command fails, a `command_error` event is also sent (tagged with the
same `operation_id`/`command` as above), breaking the failure down into:

- `kind`: the top-level error category (e.g. `internal`, `invalid-input`,
  `servicing`, `initialization`).
- `subkind`: the specific error within that category (e.g.
  `check-root-privileges`), when one applies.
- `location`: the `file:line` in Trident's source where the error was
  originally raised.

## Delivery

Telemetry delivery is always best-effort and never affects servicing
outcomes, but failures are not all logged at the same level: a failure to
serialize an event, or to enqueue it because the background uploader has
already shut down, is logged at trace level, while a failure to actually
deliver an event (e.g. no network connectivity, or a non-2xx response from
Application Insights) is logged at error level, so operators can find
remote-delivery problems in normal logs.
