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

Telemetry defaults to **disabled** (`OptOut`). To enable it, add a line to
the Agent Configuration file:

``` conf
Telemetry=OptIn
```

Any value other than `OptIn` (including an absent `Telemetry` line) is
treated as `OptOut`. Telemetry delivery is always best-effort and never
affects servicing outcomes, but failures are not all logged at the same
level: a failure to serialize an event, or to enqueue it because the
background uploader has already shut down, is logged at trace level,
while a failure to actually deliver an event (e.g. no network
connectivity, or a non-2xx response from Application Insights) is
logged at error level, so operators can find remote-delivery problems
in normal logs.
