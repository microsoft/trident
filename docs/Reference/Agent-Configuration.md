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

The `Telemetry` setting records the preference for sending data to
Application Insights. Recording this preference alone does not upload
data: delivery also requires Application Insights runtime integration
and a connection string compiled into the binary using
`AZURE_MONITOR_CONNECTION_STRING`.

The preference defaults to **OptOut**. To record an opt-in preference,
add a line to the Agent Configuration file:

``` conf
Telemetry=OptIn
```

The value is case-insensitive (`OptIn`, `optin`, and `OPTIN` are all
equivalent); any value other than a case-insensitive match for `OptIn`
(including an absent `Telemetry` line) is treated as `OptOut`.
