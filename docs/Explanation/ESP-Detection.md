
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

## ESP Detection in COSI-Derived Host Configuration

When Trident derives the Host Configuration from a COSI image (rather than
reading it from user-provided YAML), the ESP detection mechanism is different.

In this path, Trident reads GPT partition metadata from the image and matches
each partition against its corresponding filesystem metadata. A partition is
identified as the ESP if:

1. Its GPT partition type is `ESP`
   (`C12A7328-F81F-11D2-BA4B-00A0C93EC93B`), **and**
2. The associated filesystem has a **non-empty** mount point.

If multiple ESP partitions with mount points are found, the first one
encountered is used as the canonical ESP and a warning is logged for the rest.
If no qualifying ESP partition is found, the derivation fails.

:::note
In COSI-derived configurations, the ESP is detected from the GPT partition type
rather than by matching a specific mount point path. This differs from the
user-authored Host Configuration YAML path, where detection is based on the
mount point and the `overrideEspMount` field.
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
