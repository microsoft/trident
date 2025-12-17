# Runtime Updates

Trident supports runtime updates, which allow certain configuration changes to
be applied to the running operating system without requiring a full OS image
update. Runtime updates are faster and less disruptive than A/B updates because
they only modify specific components rather than provisioning an entire new root
filesystem.

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
   - For sysexts and confexts, downloaded extension images from 'Stage' are
     moved to their final location and Trident calls `systemd-sysext refresh` or
     `systemd-confext refresh` to merge the new extensions into the running OS.
   - For network configuration, Trident applies the new Netplan settings.

3. **Health Checks**: If health checks are configured in the Host Configuration,
   Trident runs them to verify the update was successful.

Unlike A/B updates, runtime updates do not require a reboot. The changes take
effect immediately on the running OS.

## Rollback Support

Runtime updates support automatic rollback if health checks fail. If any
operation during Finalize fails or if a Health Check fails, an automatic
rollback occurs reverting the OS back to the state of the previous Host
Configuration.

## Separate Stage and Finalize

Like other Trident operations, runtime updates support separating the stage and
finalize operations using the
[`--allowed-operations`](../Reference/Trident-CLI.md#--allowed_operations-allowed_operations-1)
flag:

```bash
# Stage the update
sudo trident update config.yaml --allowed-operations stage

# Later, finalize the update
sudo trident update config.yaml --allowed-operations finalize
```
