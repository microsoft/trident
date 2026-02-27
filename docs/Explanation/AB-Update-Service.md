# A/B Update Service

An A/B update provisions a new OS onto the inactive
[A/B volume](../Reference/Glossary.md#ab-volume-pair) while the current OS
continues running on the active volume. Like an
[install](./Install-Service.md), it is driven by a
[Host Configuration](../Reference/Host-Configuration/API-Reference/HostConfiguration.md)
file that declares the desired state. Trident compares the new Host
Configuration against the currently provisioned one and applies only the
necessary changes to bring the inactive volume to the desired state.

An A/B update is triggered automatically when Trident detects that the Host
Configuration changes go beyond
[runtime-updateable components](./Runtime-Updates.md). For an overview of how
Trident selects the update type, see
[How Trident Knows What to Do](./How-Trident-Knows-What-to-Do.md).

## Operations

An A/B update is split into two [operations](./Operations.md):

1. **Stage** — streams new OS images to the inactive volume and configures the
   target OS. Because the active volume is untouched, the current workload
   continues running undisturbed during this phase.
2. **Finalize** — configures the UEFI BootNext variable to boot the updated
   volume on the next reboot, then triggers the reboot.

These can be run together or separately. See
[Two-Step Installation and Update](../How-To-Guides/Two-Step-Installation-and-Update.md)
for details on running them independently. Separating stage from finalize is
particularly useful for A/B updates because the time-consuming image download
can happen in the background while the workload runs, and finalize (the
disruptive reboot) can be scheduled for a maintenance window.

## What Happens During an A/B Update

### Storage

The storage subsystem streams new OS images to the inactive volume:

- **Image streaming** — new [COSI](./COSI.md) images are streamed from remote
  sources (HTTP or OCI) to the inactive partitions using the
  [image streaming pipeline](./Image-Streaming-Pipeline.md).
- **Encryption** — if the system uses LUKS encryption, the inactive volume is
  re-encrypted with updated keys as needed.
- **Verity** — dm-verity hashes are updated for [root](./Root-Verity.md) or
  [usr](./Usr-Verity.md) integrity verification.

Unlike an install, an A/B update does not create new partitions, partition
tables, RAID arrays, or A/B volume pairs. The disk layout established during
install is preserved. Features such as software RAID, ESP redundancy, and
partition adoption are not activated during an update — they carry forward from
the original install.

### Bootloader

Trident updates the bootloader configuration on the inactive volume:

- The [bootloader configuration](./Bootloader-Configuration.md) is updated to
  reflect the new OS image.
- **GRUB2** or **systemd-boot** are supported. See
  [Bootloader Configuration](./Bootloader-Configuration.md).
- **Unified Kernel Images (UKI)** are supported for combined kernel, initrd,
  and command line images signed for Secure Boot.
- The [UEFI BootNext variable](./UEFI-Variables.md) is set so the firmware
  boots the updated volume on the **next reboot only**. This is in contrast to
  an install, which sets BootOrder for all subsequent reboots. The BootOrder is
  only updated after a successful commit.

### OS Configuration

Trident enters a [deployment chroot](./Deployment-Chroot.md) on the inactive
volume to configure the new OS. The full list of supported options is defined in
the [`Os` object](../Reference/Host-Configuration/API-Reference/Os.md). Key
capabilities include:

- **Network** — applies [netplan configuration](./Network-Configuration.md).
- **SELinux** — configures [SELinux mode and policy](./SELinux-Configuration.md).
- **Initrd** — regenerates the initramfs when required (GRUB only).
- **Extensions** — deploys [system extensions (sysexts)](./Sysexts.md) and
  [configuration extensions (confexts)](./Confexts.md).

### Customization

- **Script hooks** — user-provided scripts can be executed at defined points
  during the update. See [Script Hooks](./Script-Hooks.md).

### Management

Trident records the new Host Configuration and servicing state in its
datastore, enabling the subsequent commit, rollback, and future update
operations.

## After the Update

After finalize triggers a reboot, the machine boots into the updated volume.
On the next boot:

1. **Commit** — `trident commit` validates that the system booted from the
   expected volume. On success, it promotes the updated volume to active by
   updating the UEFI BootOrder variable for all subsequent reboots.
2. **Health checks** — if [health checks](./Health-Checks.md) are configured,
   Trident runs them before committing to verify the update was successful.
3. **Rollback** — if the commit fails or health checks do not pass, Trident can
   [roll back](./Manual-Rollback.md) to the previous volume. The previous
   volume remains intact and bootable because the A/B scheme guarantees that the
   old OS is never modified during the update.

This commit-or-rollback mechanism ensures that a failed update never leaves the
system in an unbootable state.
