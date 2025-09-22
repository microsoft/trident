
# Host Configuration Validation

The Trident binary can validate the syntax and structure of a host configuration without applying it
to the host:

```
trident validate /path/to/host-configuration.yaml
```

This checks only the host configuration file itself. When Trident runs an install or update, it does
further validation to ensure that the provided configuration is compatible with the host's hardware
and current state.