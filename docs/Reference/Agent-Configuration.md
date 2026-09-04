---
sidebar_position: 5
---

# Agent Configuration

The Trident Agent Configuration file contains configuration details for Trident. It is used for all Trident commands. The Agent Configuration file path must be `/etc/trident/trident.conf`.

> In most cases, the default values of Agent Configuration are sufficient and should not need to be changed.

## Setting Custom Datastore Path

By default, Trident will use `/var/lib/trident/datastore.sqlite` as the path for the datastore. To configure a non-default path, the Agent Configuration file must contain a line defining the path like this:

``` conf
DatastorePath=/special/path/to/my-datastore.sqlite
```

> The datastore path cannot be hosted on an [A/B volume pair](./Glossary#ab-volume-pair) and must be an absolute path.

## Telemetry

Trident can optionally send a best-effort stream of its tracing data (the
same metrics/spans it already records locally to `/var/log/trident-metrics.jsonl`)
to Azure Monitor / Application Insights. This requires an Application
Insights connection string to have been compiled into the Trident binary at
build time (via the `AZURE_MONITOR_CONNECTION_STRING` environment variable);
if no connection string was compiled in, this setting has no effect.

Every event sent also includes the following host metadata, so operators
should be aware this leaves the host along with the metrics/spans
themselves:

- `asset_id`: the host's DMI product UUID (a stable hardware identifier).
- `os_release`: the `VERSION` field from `/etc/os-release`.
- `kernel_version`: the running kernel release (`uname -r`).
- `total_cpu`: the number of CPUs.
- `total_memory_gib`: total memory, in GiB.
- `trident_version`: the running Trident version.
- `correlation_id`: a random ID generated once and persisted in the
  datastore, unique to this host installation. It is not derived from any
  hardware/user identifier, but because it is stable across every Trident
  invocation on this host, it does let separate events be correlated back
  to the same installation over time.
- `operation_id`: a fresh, random ID generated for each individual command
  invocation (e.g. one `trident update` run, or one gRPC request handled
  by `tridentd`). Unlike `correlation_id`, this is never reused across
  invocations -- it only lets events emitted *during the same command* be
  correlated with each other.
- `command`: which command produced the event (e.g. `install`, `update`,
  `update_stage`, `update_finalize`, `commit`, `rollback`, `rebuild_raid`).
  The `_stage`/`_finalize` suffixes distinguish a two-step (stage-only or
  finalize-only) invocation from a single combined one.

If a command fails, a `command_error` event is also sent (tagged with the
same `operation_id`/`command` as above), breaking the failure down into:

- `kind`: the top-level error category (e.g. `internal`, `invalid-input`,
  `servicing`, `initialization`).
- `subkind`: the specific error within that category (e.g.
  `check-root-privileges`), when one applies.
- `location`: the `file:line` in Trident's source where the error was
  originally raised.

Telemetry defaults to **disabled** (`OptOut`). To enable it, add a line to
the Agent Configuration file:

``` conf
Telemetry=OptIn
```

The value is case-insensitive (`OptIn`, `optin`, and `OPTIN` are all
equivalent); any value other than a case-insensitive match for `OptIn`
(including an absent `Telemetry` line) is treated as `OptOut`. Telemetry
delivery is always best-effort and never
affects servicing outcomes, but failures are not all logged at the same
level: a failure to serialize an event, or to enqueue it because the
background uploader has already shut down, is logged at trace level,
while a failure to actually deliver an event (e.g. no network
connectivity, or a non-2xx response from Application Insights) is
logged at error level, so operators can find remote-delivery problems
in normal logs.
