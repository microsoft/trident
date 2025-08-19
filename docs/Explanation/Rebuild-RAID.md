# Rebuild RAID
## Table of Contents

- [Table of Contents](#table-of-contents)
  - [RAID and Rebuild-RAID](#raid-and-rebuild-raid)
  - [Use Cases](#use-cases)
    - [When Should This Feature Be Used?](#when-should-this-feature-be-used)
    - [When Is It Valuable?](#when-is-it-valuable)
    - [When Is It Not?](#when-is-it-not)
  - [Capabilities \& Limitations](#capabilities--limitations)
    - [Capabilities](#capabilities)
    - [Limitations](#limitations)
  - [Implementation](#implementation)
    - [What Is Trident Doing Internally](#what-is-trident-doing-internally)
    - [Sample Host Configuration](#sample-host-configuration)

## RAID and Rebuild-RAID

RAID (Redundant Array of Independent Disks) is a technology that uses multiple
disks to provide fault tolerance, improve performance, or both. When a disk in a
RAID array fails, RAID-rebuild is the process by which the data that was on the
failed disk is reconstructed onto a new disk.

## Use Cases

### When Should This Feature Be Used?

- When a disk in a RAID array fails and needs to be replaced.

### When Is It Valuable?

- In environments where data loss cannot be tolerated.

### When Is It Not?

- For single-disk systems where RAID is not implemented.
- In systems where a different data redundancy or backup strategy is in use.

## Capabilities & Limitations

### Capabilities

- Reconstruction of data from a failed disk onto a new disk.
- Maintaining data integrity and system functionality during the rebuild process.

### Limitations

- Cannot handle the detection and physical replacement of the failed disk.
- Rebuild RAID is not supported if the disk configuration includes images, mount points, or encryption on partitions which are not members of RAID.
**Disk Configuration Requirements**: The new disk for the rebuild must only have unformatted partitions or partitions which are members of software RAID arrays. An unformatted partition is a segment of a disk that has not been formatted with a file system; it is in its initial, unprocessed state. Rebuilding is not supported if the disk configuration includes images, mount points, or encryption on partitions which are not members of RAID.

**Consistency with Initial Host Configuration**: The disk configuration must match the original host configuration provided when the host was first provisioned. Simply run `trident rebuild-raid`, and Trident will by default load its configuration from `/etc/trident/config.yaml` to start rebuilding the RAID arrays on the new disk.

## Implementation

This is a step-by-step explanation of how RAID-rebuild works, using RAID 1 (mirroring) as an example:

1. **Detection**: The RAID controller or user software identifies the failed disk.
2. **Replacement**: The user physically replaces the failed disk with a new, identical (or larger) one.
3. **Trident RAID-Rebuild**: This step will start the process of reconstructing the data and configuration of the removed disk onto the new disk, ensuring data integrity and system functionality are maintained.

**Note**: Trident does not handle Steps 1 and 2; these must be performed by the user.

Please refer to [Trident Rebuild RAID](../How-To-Guides/Trident-Rebuild-RAID.md) for more details on how to run Trident Rebuild-RAID.

### What Is Trident Doing Internally

When the `trident rebuild-raid` command is executed, it first verifies whether
the host configuration is suitable for a rebuild operation. It identifies the
presence of a new disk by monitoring the UUIDs of the disks. If a new disk is
detected, it proceeds with further validation.

The command then examines whether the disk configuration includes members of a
RAID array that have an active copy, enabling data recovery from another disk,
or if it has unformatted partitions.

If the rebuild operation is deemed feasible, Trident begins by rebuilding the
new disk. It creates partitions and integrates the newly created RAID array
members into the existing RAID array.

### Sample Host Configuration

```yaml
storage:
  disks:
    - id: disk1
      device: /dev/disk/by-path/pci-0000:00:1f.2-ata-2
      partitionTableType: gpt
      partitions:
        - id: esp1
          type: esp
          size: 100M
        - id: root-a1
          type: root
          size: 4G
        - id: root-b1
          type: root
          size: 1G
    - id: disk2
      device: /dev/disk/by-path/pci-0000:00:1f.2-ata-3
      partitionTableType: gpt
      partitions:
        - id: root-a2
          type: root
          size: 4G
        - id: root-b2
          type: root
          size: 1G
        - id: raw-part
          type: linux-generic
          size: 100M
  raid:
    software:
      - id: root-a
        name: root-a
        level: raid1
        devices:
          - root-a1
          - root-a2
      - id: root-b
        name: root-b
        level: raid1
        devices:
          - root-b1
          - root-b2
  abUpdate:
    volumePairs:
      - id: root
        volumeAId: root-a
        volumeBId: root-b
  filesystems:
    - deviceId: root
      type: ext4
      source:
        type: image
        url: http://NETLAUNCH_HOST_ADDRESS/files/root.rawzst
        sha256: ignored
        format: raw-zst
      mountPoint:
        path: /
        options: defaults
    - deviceId: esp1
      type: vfat
      source:
        type: esp-image
        url: http://NETLAUNCH_HOST_ADDRESS/files/esp.rawzst
        sha256: ignored
        format: raw-zst
      mountPoint:
        path: /boot/efi
        options: umask=0077
```

In the sample host configuration above, a Trident rebuild RAID operation can be
initiated if **disk2** fails and is replaced with a new disk. Using the RAID
rebuild feature, the new disk can be reconstructed if recoverable copies of the
RAID partitions are available on other disks. In this configuration, we see that
the partitions `root-a2` and `root-b2` are part of the RAID arrays `root-a` and
`root-b`, respectively. The data on these partitions can be rebuilt on the new
disk because mirrored copies (`root-a1` and `root-a2`) are present on disk1.
Additionally, disk2 contains an unformatted partition named `raw-part`, and we
support initiating a rebuild if unformatted partitions are present. However, we
can only restore data on RAID-ed partitions but not on the unformatted
partitions using the Trident rebuild RAID feature.

**Note**: When disk2 is replaced, the new disk should be a similar device. In
the above example, the new disk should have the device attribute
`/dev/disk/by-path/pci-0000:00:1f.2-ata-3`.

In the above example, if **disk1** fails and is replaced, we do not support
Trident RAID rebuilding because the partition `esp1` is neither part of a
recoverable RAID array nor a raw partition.
