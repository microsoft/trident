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
- `database_id`: an ID that lets separate events be correlated back to the
  same datastore over its entire lifetime (generated on first access to
  the datastore, whether or not an install has actually happened yet).
- `installation_id`: an ID that lets separate events be correlated back to
  the same host installation over time. Unlike `database_id`, this is
  only ever created (get-or-create, never overwritten) at the start of
  `Trident::install`, so it is absent from any event that fires before a
  host's first-ever install.
- `operation_id`: an ID that lets events emitted during the same command
  invocation be correlated with each other.
- `command`: which command produced the event (e.g. `install`, `update`,
  `update_stage`, `update_finalize`, `commit`, `rollback`, `rebuild_raid`).

## Delivery

Telemetry delivery is always best-effort and never affects servicing
outcomes, but failures are not all logged at the same level: a failure to
serialize an event, or to enqueue it because the background uploader has
already shut down, is logged at trace level, while a failure to actually
deliver an event (e.g. no network connectivity, or a non-2xx response from
Application Insights) is logged at error level, so operators can find
remote-delivery problems in normal logs.
