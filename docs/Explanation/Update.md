# Update

An is a servicing type that applies changes to an existing Trident-managed
system. Like an [install](./Install.md), it is driven by a [Host
Configuration](../Reference/Host-Configuration/API-Reference/HostConfiguration.md)
file that declares the desired state. Trident compares the new Host
Configuration against the currently provisioned one and automatically selects
the appropriate update strategy.

For an overview of how Trident determines what to do based on the Host
Configuration, see
[How Trident Knows What to Do](./How-Trident-Knows-What-to-Do.md).

## Update Types

Trident supports two update strategies, selected automatically based on what
has changed in the Host Configuration:

### A/B Update

An [A/B update](../Reference/Glossary.md#ab-update) is used when the OS image
or any non-runtime-updateable configuration has changed. It provisions a
complete new OS onto the inactive [A/B volume](../Reference/Glossary.md#ab-volume-pair)
while the current OS continues running, then reboots into the updated volume.
See [A/B Update](./AB-Update.md) for a detailed breakdown of
what happens during an A/B update.

### Runtime Update

A [runtime update](./Runtime-Updates.md) is used when only runtime-updateable
components have changed — currently
[sysexts](./Sysexts.md), [confexts](./Confexts.md), and
[network configuration](./Network-Configuration.md). Runtime updates apply
changes directly to the running OS without a reboot, making them faster and
less disruptive.

If any other part of the Host Configuration has changed, Trident will
automatically perform an A/B update instead of a runtime update.

## Operations

Like an install, an update is split into two [operations](./Operations.md):

1. **Stage** — streams new OS images to the inactive volume (A/B) or downloads
   new extensions (runtime). The current workload continues running undisturbed.
2. **Finalize** — for A/B updates, configures the UEFI BootNext variable and
   reboots into the updated volume. For runtime updates, activates the changes
   on the running OS without a reboot.

These can be run together or separately. See
[Two-Step Installation and Update](../How-To-Guides/Two-Step-Installation-and-Update.md)
for details on running them independently.
