---
sidebar_position: 5
---

# Agent Configuration

The Trident Agent Configuration file contains configuration details for Trident. It is used for all Trident commands. The Agent Configuration file path must be `/etc/trident/trident.conf`.

> In most cases, the default values of Agent Configuration are sufficient and should not need to be changed.

If the Agent Configuration file is not found, Trident uses the default settings.
Other read failures, including permission errors and invalid UTF-8, return an
error instead of silently falling back to defaults.

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

Telemetry defaults to **enabled** (`OptIn`). To disable it, add a line to
the Agent Configuration file:

``` conf
Telemetry=OptOut
```

The value is case-insensitive (`OptIn`, `optin`, and `OPTIN` are all
equivalent). A missing file or absent `Telemetry` line uses `OptIn`;
unrecognized values disable telemetry defensively.

Alternatively, install the opt-out RPM:

``` bash
tdnf install trident-telemetry-optout
```

Installing or upgrading this package sets `Telemetry=OptOut`, creating
`/etc/trident/trident.conf` if needed and preserving all other settings.
The configuration is left in place when the package is removed; to
re-enable telemetry, set `Telemetry=OptIn` explicitly.
Changes take effect on the next Trident invocation; restart a running
Trident daemon to apply them there.
