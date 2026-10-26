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

Trident can optionally send telemetry data to Azure Monitor / Application
Insights. See [Telemetry](./Telemetry.md) for what data is collected and
how delivery failures are handled. This requires an Application Insights
connection string to have been compiled into the Trident binary at build
time (via the `AZURE_MONITOR_CONNECTION_STRING` environment variable); if
no connection string was compiled in, this setting has no effect.

Telemetry defaults to **disabled** (`OptOut`). To enable it, add a line to
the Agent Configuration file:

``` conf
Telemetry=OptIn
```

The value is case-insensitive (`OptIn`, `optin`, and `OPTIN` are all
equivalent); any value other than a case-insensitive match for `OptIn`
(including an absent `Telemetry` line) is treated as `OptOut`.
