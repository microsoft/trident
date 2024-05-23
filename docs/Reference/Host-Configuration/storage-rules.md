# Storage Configuration Rules

Documentation about the rules used to validate the storage configuration.

## Contents

- [Reference Validity](#reference-validity)
- [Reference Count](#reference-count)
- [Reference Sharing](#reference-sharing)
- [Filesystem Block Device Requirements](#filesystem-block-device-requirements)
- [Filesystem Source Requirements](#filesystem-source-requirements)
- [Filesystem Mounting](#filesystem-mounting)
- [Filesystem Verity Support](#filesystem-verity-support)
- [Homogeneous References](#homogeneous-references)
- [Homogeneous Partition Types](#homogeneous-partition-types)
- [Homogeneous Partition Sizes](#homogeneous-partition-sizes)
- [Allowed Partition Types](#allowed-partition-types)
- [Partition Type Valid Mounting Paths](#partition-type-valid-mounting-paths)
- [Partition Type Matching Hash Partition](#partition-type-matching-hash-partition)

## Reference Validity

This table contains the rules for valid references in the storage configuration.
The rows represent the different types of referrers that exists in the
configuration, and the columns replresent the different types of block devices
that can be referenced.

A single cell in the table represents whether a referrer of a certain type can
reference a block device of a certain type.

| Referrer \ Device      | disk | partition | raid-array | ab-volume | encrypted-volume |
| ---------------------- | ---- | --------- | ---------- | --------- | ---------------- |
| raid-array             | No   | Yes       | No         | No        | No               |
| ab-volume              | No   | Yes       | Yes        | No        | Yes              |
| encrypted-volume       | No   | Yes       | Yes        | No        | No               |
| filesystem             | No   | Yes       | Yes        | Yes       | Yes              |
| filesystem-adopted     | No   | No        | No         | No        | No               |
| verity-filesystem-data | No   | Yes       | Yes        | Yes       | No               |
| verity-filesystem-hash | No   | Yes       | Yes        | Yes       | No               |

## Reference Count

A referrer may only reference a certain number of block devices. The table below
shows valid reference counts for each referrer type.

| Referrer type          | Min | Max |
| ---------------------- | --- | --- |
| raid-array             | 2   | ∞   |
| ab-volume              | 2   | 2   |
| encrypted-volume       | 1   | 1   |
| filesystem             | 0   | 1   |
| filesystem-adopted     | 1   | 1   |
| verity-filesystem-data | 1   | 1   |
| verity-filesystem-hash | 1   | 1   |

## Reference Sharing

Mostg referrers claim exlusive access over their references. This table contains
the rules for sharing references in the storage configuration.

| Referrer type          | Valid sharing peers |
| ---------------------- | ------------------- |
| raid-array             | (none)              |
| ab-volume              | (none)              |
| encrypted-volume       | (none)              |
| filesystem             | (none)              |
| filesystem-adopted     | (none)              |
| verity-filesystem-data | (none)              |
| verity-filesystem-hash | (none)              |

## Filesystem Block Device Requirements

Depending on the type of a filesystem, they may or may nor reference a backing
block device.

| File System Type | Requires Block Device |
| ---------------- | --------------------- |
| ext4             | Yes                   |
| xfs              | Yes                   |
| vfat             | Yes                   |
| swap             | Yes                   |
| tmpfs            | No                    |
| auto             | Yes                   |

## Filesystem Source Requirements

Depending on the type of a filesystem, they may have different source types.

| File System Type | Valid Source Type          |
| ---------------- | -------------------------- |
| ext4             | create or image or adopted |
| xfs              | create or image or adopted |
| vfat             | create or image or adopted |
| swap             | create                     |
| tmpfs            | create                     |
| auto             | adopted                    |

## Filesystem Mounting

Depending on the type of a filesystem, they may or may not have a mountpoint
configured.

| File System Type | Can Have Mount Point |
| ---------------- | -------------------- |
| ext4             | Yes                  |
| xfs              | Yes                  |
| vfat             | Yes                  |
| swap             | No                   |
| tmpfs            | Yes                  |
| auto             | Yes                  |

## Filesystem Verity Support

Depending on the type of a filesystem, they may or may not be used for verity.

| File System Type | Supports Verity |
| ---------------- | --------------- |
| ext4             | Yes             |
| xfs              | Yes             |
| vfat             | No              |
| swap             | No              |
| tmpfs            | No              |
| auto             | No              |

## Homogeneous References

The following referrers require that all referenced devices are of the same type:

- raid-array
- ab-volume
- encrypted-volume
- verity-filesystem-data

## Homogeneous Partition Types

The following referrers require that all underlying partitions are of the same type:

- raid-array
- ab-volume

## Homogeneous Partition Sizes

The following referrers require that all underlying partitions are of the same size:

- raid-array

## Allowed Partition Types

Some referrers only support specific underlying partitions types.

| Referrer type          | Allowed partition types                    |
| ---------------------- | ------------------------------------------ |
| raid-array             | any                                        |
| ab-volume              | any                                        |
| encrypted-volume       | any type except esp or root or root-verity |
| filesystem             | any                                        |
| filesystem-adopted     | any                                        |
| verity-filesystem-data | root                                       |
| verity-filesystem-hash | root-verity                                |

## Partition Type Valid Mounting Paths

This rule is not strictly enforced, but is provided as a warning to the user.

Some partition types have expected mount point paths, and will generally be
mounted at the expected path. For example, the `boot` partition is generally
mounted at `/boot`, and the `root` partition is generally mounted at `/`.

The following table lists the expected mount points for each partition type, as
defined in the [Discoverable Partition Specification
(DPS)](https://uapi-group.org/specifications/specs/discoverable_partitions_specification/):

| Mount Path    | Valid Mount Paths                |
| ------------- | -------------------------------- |
| esp           | `/boot` or `/efi` or `/boot/efi` |
| root          | `/`                              |
| swap          | None                             |
| root-verity   | None                             |
| home          | `/home`                          |
| var           | `/var`                           |
| usr           | `/usr`                           |
| tmp           | `/var/tmp`                       |
| linux-generic | Any path                         |
| srv           | `/srv`                           |
| xbootldr      | `/boot`                          |

## Partition Type Matching Hash Partition

Partitions being used for verity need a matching partition for the hash data.

This table lists the expected hash partition for each partition type.
Types that are not listed are not valid for verity.

| Partition Type | Matching Hash Partition |
| -------------- | ----------------------- |
| root           | root-verity             |

