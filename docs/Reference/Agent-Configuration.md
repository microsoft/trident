
# Agent Configuration

The Trident Agent Configuration file contains configuration details for Trident.  It is used across invocations for all Trident commands.  The Agent Configuration file path must be `/etc/trident/trident.conf`.

## Setting customized datastore path

By default, Trident will use `/var/lib/trident/datastore.sqlite` as the path for the datastore.  To configure a non-default path, the Agent Configuration file must contain a line defining the path like this:

```
DatastorePath=/special/path/to/my-datastore.sqlite
```

