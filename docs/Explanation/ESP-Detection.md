
# ESP Detection

The EFI System Partition (ESP) is a critical component of UEFI-based systems.
It contains the bootloader, kernel images, and other files required to boot the
operating system. Trident needs to know the ESP mount point path so it can
manage bootloader files, stage UKI images, configure UEFI fallback, and
generate the correct fstab entries.

By default, if no additional configuration is provided, Trident assumes the ESP
is mounted at `/boot/efi`.

There are two distinct paths through which Trident identifies the ESP:

1. **Host Configuration** — The user (or automation) declares filesystems
   and Trident determines which one is the ESP based on its mount point and
   the
   [`overrideEspMount`](../Reference/Host-Configuration/API-Reference/OverrideEspMount.md)
   setting.
2. **OS image (COSI) metadata** — The COSI image carries its own partition
   and filesystem metadata, including which partition is the ESP and where
   it is mounted. This is determined by the distribution and image build
   tooling.

These two sources **must agree**. During cross-validation, Trident compares the
ESP mount point declared in the Host Configuration against the ESP mount point
found in the OS image metadata. If they differ, Trident rejects the
configuration with an `EspMountPointMismatch` error. This means that when
authoring a Host Configuration for a given distribution, the ESP mount point
must match the path the distribution's COSI image expects.

## ESP Detection in Host Configuration YAML

### Default Behavior

When a filesystem entry is declared in the Host Configuration and no
`overrideEspMount` field is specified, Trident applies the `use-default`
behavior: the filesystem is treated as the ESP **if and only if** its mount
point path is `/boot/efi`.

For example, this filesystem is automatically detected as the ESP:

``` yaml
storage:
  filesystems:
  - deviceId: esp
    mountPoint: /boot/efi
```

No additional configuration is needed for distributions that mount the ESP at
the standard `/boot/efi` path.

### Overriding with `overrideEspMount`

Some Linux distributions mount the ESP at a different path, such as `/boot` or
`/efi`. To support these layouts, the
[`overrideEspMount`](../Reference/Host-Configuration/API-Reference/OverrideEspMount.md)
field on a
[`FileSystem`](../Reference/Host-Configuration/API-Reference/FileSystem.md)
entry allows explicit control over ESP detection.

The field accepts three values:

| Value         | Behavior                                                                                                |
| ------------- | ------------------------------------------------------------------------------------------------------- |
| `use-default` | *(Default)* The filesystem is the ESP only if its mount point is `/boot/efi`.                           |
| `override`    | The filesystem is treated as the ESP regardless of its mount point path. A mount point must be present. |
| `block`       | The filesystem is **not** the ESP, even if it is mounted at `/boot/efi`.                                |

#### Example: ESP at `/efi`

``` yaml
storage:
  filesystems:
  - deviceId: esp
    overrideEspMount: override
    mountPoint: /efi
```

#### Example: ESP at `/boot`

When mounting the ESP at `/boot`, ensure that no separate `/boot` filesystem is
also declared, as two filesystems cannot share the same mount point.

``` yaml
storage:
  filesystems:
  - deviceId: esp
    overrideEspMount: override
    mountPoint: /boot
  - deviceId: root
    mountPoint: /
```

### Blocking False Positives

In rare cases, a distribution may have a non-ESP filesystem mounted at
`/boot/efi`. Without intervention, Trident would incorrectly treat that
filesystem as the ESP. The `block` value prevents this.

:::note
When using `block`, the actual ESP filesystem **must** be explicitly marked
with `override`. Otherwise, Trident will not find an ESP and validation will
fail.
:::

``` yaml
storage:
  filesystems:
  # This filesystem is NOT the ESP, despite being at /boot/efi.
  - deviceId: not-esp
    overrideEspMount: block
    mountPoint: /boot/efi
  # This is the real ESP, mounted at a non-default path.
  - deviceId: real-esp
    overrideEspMount: override
    mountPoint: /efi
```

### Detection Summary

The following table summarizes how the `overrideEspMount` value and mount point
path interact to determine whether a filesystem is detected as the ESP:

| `overrideEspMount` | Mount Point     | Detected as ESP? | Notes                                                    |
| ------------------ | --------------- | ---------------- | -------------------------------------------------------- |
| `use-default`      | `/boot/efi`     | Yes              | Default path matches automatically.                      |
| `use-default`      | `/efi`          | No               | Path does not match the default.                         |
| `use-default`      | `/boot`         | No               | Path does not match the default.                         |
| `override`         | `/efi`          | Yes              | Explicitly marked as ESP.                                |
| `override`         | `/boot`         | Yes              | Explicitly marked as ESP.                                |
| `override`         | *(not set)*     | Error            | `override` requires a mount point.                       |
| `override`         | `/boot/efi`     | Yes              | Still treated as ESP, even though it's the default path. |
| `block`            | `/boot/efi`     | No               | Blocked from being detected, even at default path.       |
| `block`            | (anything else) | No               | Not detected as ESP regardless of path.                  |

## Validation Rules

After ESP detection, the storage configuration is validated. The following rules
are enforced:

- **Exactly one ESP.** There must be exactly one filesystem marked as the ESP.
  Having zero or more than one is an error.
- **ESP must have a mount point.** A filesystem marked as the ESP without a
  mount point is rejected.
- **Expected partition types.** When the ESP filesystem is backed by a
  partition, that partition is expected to use the `esp` partition type
  as defined by the
  [Discoverable Partitions Specification](https://uapi-group.org/specifications/specs/discoverable_partitions_specification/).

Additionally, the following mount paths are recognized as valid locations for
an `esp`-typed partition. Using a different path produces a warning but does
not cause validation failure:

- `/boot/efi`
- `/boot`
- `/efi`

For the full set of storage validation rules, see
[Storage Rules](../Reference/Host-Configuration/Storage-Rules.md).

## ESP in OS Image (COSI) Metadata

The OS image (COSI) carries its own filesystem and partition metadata, which
reflects how the distribution was built. The ESP mount point in the image is
determined by the distribution and image build tooling — for example, a
distribution that mounts the ESP at `/boot` will produce a COSI image with
the ESP filesystem metadata pointing to `/boot`.

When Trident derives a Host Configuration automatically from a COSI image
(rather than reading user-provided YAML), it identifies the ESP by reading GPT
partition metadata from the image. A partition is treated as the ESP if:

1. Its GPT partition type is `ESP`
   (`C12A7328-F81F-11D2-BA4B-00A0C93EC93B`), **and**
2. The associated filesystem has a **non-empty** mount point.

If multiple ESP partitions with mount points are found, the first one
encountered is used as the canonical ESP and a warning is logged for the rest.
If no qualifying ESP partition is found, the derivation fails.

### Cross-Validation: Host Configuration Must Match the Image

When a user provides their own Host Configuration (the common case), Trident
performs a cross-validation step that compares the ESP mount point in the Host
Configuration against the ESP mount point in the OS image metadata. If they do
not match, Trident rejects the configuration with an `EspMountPointMismatch`
error.

This means that the `overrideEspMount` and `mountPoint` settings in the Host
Configuration must reflect the actual ESP layout of the distribution's COSI
image. For example, if a distribution's image places the ESP at `/efi`, the
Host Configuration must use `overrideEspMount: override` with
`mountPoint: /efi`.

:::note
The cross-validation check ensures that Trident, the Host Configuration, and
the OS image all agree on where the ESP lives. A mismatch typically indicates
that the Host Configuration was written for a different distribution or image
layout than the one being deployed.
:::

## How the Detected ESP Path Is Used

After validation, Trident stores the resolved ESP mount point path and makes it
available to all boot-related subsystems. This path is used for:

- **Bootloader layout** — GRUB and systemd-boot configuration files are written
  under the ESP mount path (e.g., `<esp>/EFI/...`).
- **UKI staging** — Unified Kernel Images are placed in the correct ESP
  directory structure.
- **UEFI fallback** — Boot files are copied to the UEFI fallback path on the
  ESP. See [UEFI Fallback](./UEFI-Fallback.md) for more details.
- **fstab generation** — The ESP mount point is included in the generated fstab
  for the target OS.
- **Bootloader servicing** — A/B update bootloader management uses the ESP path
  to locate and manage boot entries. See
  [Bootloader Configuration](./Bootloader-Configuration.md) for more details.
