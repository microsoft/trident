# Runtime Updates

Trident supports runtime updates, which allow certain configuration changes to
be applied to the servicing OS without requiring a full OS image update. Note
that in the case of runtime updates, the [servicing
OS](../Reference/Glossary.md#servicing-os) is the same as the [target
OS](../Reference/Glossary.md#target-os). Runtime updates are faster and less
disruptive than A/B updates because they only modify specific components rather
than provisioning an entire new root filesystem, and therefore do not require
rebooting the host.

Runtime updates are triggered automatically by the
[`update`](../Reference/Trident-CLI.md#update) command when Trident detects that
only runtime-updateable components have changed in the Host Configuration.

## Supported Configurations

Runtime updates currently support the following configurations:

- [**System Extensions
  (sysexts)**](../Reference/Host-Configuration/API-Reference/Os.md#sysexts-optional):
- [**Configuration Extensions
  (confexts)**](../Reference/Host-Configuration/API-Reference/Os.md#confexts-optional):
- [**Network Configuration
  (netplan)**](../Reference/Host-Configuration/API-Reference/Os.md#netplan-optional):

If any other part of the Host Configuration has changed, Trident will begin an
[A/B update](../Reference/Glossary.md#ab-update) instead of a runtime update.

## How Runtime Updates Work

When you run `trident update` with a Host Configuration that only changes
runtime-updateable components:

1. **Stage**: Trident downloads any new sysext or confext images and validates
   them. Network configuration changes are prepared but not yet applied.

2. **Finalize**: Trident activates the changes:
   - For sysexts and confexts, downloaded extension images from **Stage** are
     moved to their final location and Trident calls `systemd-sysext refresh` or
     `systemd-confext refresh` to merge the new extensions into the running OS.
   - For network configuration, Trident applies the new Netplan settings.

3. **Health Checks**: If health checks are configured in the Host Configuration,
   Trident runs them to verify the update was successful. Ensure that the health
   check is configured to run on runtime updates by specifiying `runtime-update`
   after `runsOn`.

Unlike A/B updates, runtime updates do not require a reboot. The changes take
effect immediately on the running OS.

## Rollback Support

If an operation during 'Finalize' produces an error or if a health check fails,
an automatic rollback occurs, reverting the OS back to the state of the previous
Host Configuration.

## Separate Stage and Finalize

Runtime updates may be separated into stage and finalize operations using the
[`--allowed-operations`](../Reference/Trident-CLI.md#--allowed_operations-allowed_operations-1)
flag:

```bash
# Stage the update
sudo trident update config.yaml --allowed-operations stage

# Later, finalize the update
sudo trident update config.yaml --allowed-operations finalize
```

Separating `stage` from `finalize` allows you to handle the often more
time-consuming download extension images in advance, allowing you to quickly
apply the update later by running only `finalize`.

## Known Issues

Runtime updates of `netplan` are not compatible with root-verity, since
Trident's implementation of root-verity mounts a read-only overlay over `/etc`.
