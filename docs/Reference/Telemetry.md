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
themselves. `trident_version`, `os_release`, `kernel_version`, `total_cpu`,
and `total_memory_gib` are always present; the remaining fields are only
attached once their respective ID has actually been minted or the event is
emitted from within a command, as noted below:

- `os_release`: the `VERSION` field from `/etc/os-release`.
- `kernel_version`: the running kernel release (`uname -r`).
- `total_cpu`: the number of CPUs.
- `total_memory_gib`: total memory, in GiB.
- `trident_version`: the running Trident version.
- `installation_id`: an ID that lets separate events be correlated back
  to the same host installation over time. Present from `command_start`
  onward on every `trident install` and `trident update` invocation that
  can create or already has a datastore, including the very first one on
  a brand-new host: it's minted before the command's own telemetry
  begins, not deferred until after the install succeeds. It's likewise
  present on the first command run against a datastore that was
  provisioned some other way (e.g. offline initialization), as a
  one-time migration. Not present on events emitted before a datastore
  exists at all, such as a `get`/`diagnose` run against an unprovisioned
  host, a rejected `install --config`, or a finalize-only invocation
  (`--allowed-operations finalize`) against an unprovisioned host (no
  datastore file yet). Also
  deliberately left unattached for the earliest events (up to and
  including `trident_start`) of a **multiboot** install: the correct
  datastore for that invocation isn't known until `Trident::install`
  decides whether to swap to a brand-new temporary one, so attaching an
  ID any earlier risks stamping those events with the wrong (pre-swap)
  host's ID instead.
- `servicing_id`: an ID that lets events from a single servicing
  operation (install, update, or manual rollback) be correlated with
  each other, even across a reboot between staging and finalizing.
  Present once that operation has been confirmed as real (i.e. it isn't
  a no-op and its servicing type is valid for the command) but *before*
  any preflight work that can itself fail -- pre-servicing hooks, Host
  Configuration validation, filesystem population -- so a failure in any
  of those is also correlated by servicing_id. Not present on events
  emitted before that point (e.g. a no-op update with nothing to do, or a
  rejected gRPC request) or on commands that never stage anything, such
  as `get` or `diagnose`.
- `operation_id`: an ID that lets events emitted during the same command
  invocation be correlated with each other. Present on every event
  emitted from within a command's tracked execution (from
  `command_start` onward); not present on events emitted outside any
  command context.
- `command`: which command produced the event (e.g. `install`, `update`,
  `update_stage`, `update_finalize`, `commit`, `rollback`, `rebuild_raid`).
  The `_stage`/`_finalize` suffixes distinguish a two-step (stage-only or
  finalize-only) invocation from a single combined one; a `_noop` suffix
  (e.g. `install_noop`, `update_noop`) instead marks an invocation where
  neither stage nor finalize was requested (`--allowed-operations` with
  both disabled). Present under the
  same condition as `operation_id`.

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
