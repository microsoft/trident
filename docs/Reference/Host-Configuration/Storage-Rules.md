# Storage Configuration Rules

Documentation about the rules used to validate the storage configuration.

## Contents

- [Block Device Description](#block-device-description)
- [Referrer Description](#referrer-description)
- [Reference Validity](#reference-validity)
- [Reference Count](#reference-count)
- [Reference Sharing](#reference-sharing)
- [Unique Field Value Constraints](#unique-field-value-constraints)
- [Filesystem Block Device Requirements](#filesystem-block-device-requirements)
- [Filesystem Source Requirements](#filesystem-source-requirements)
- [Filesystem Mounting](#filesystem-mounting)
- [Filesystem Verity Support](#filesystem-verity-support)
- [Homogeneous References](#homogeneous-references)
- [Homogeneous Partition Types](#homogeneous-partition-types)
- [Homogeneous Partition Sizes](#homogeneous-partition-sizes)
- [Allowed Partition Types](#allowed-partition-types)
- [Allowed RAID Levels](#allowed-raid-levels)
- [Partition Type Valid Mounting Paths](#partition-type-valid-mounting-paths)
- [Partition Type Matching Hash Partition](#partition-type-matching-hash-partition)

## Block Device Description

This table lists all the different kinds of block devices that exist in the
configuration, along with their descriptions.

| Block device kind | Description                                                                  |
| ----------------- | ---------------------------------------------------------------------------- |
| none              | Represents a 'null device' i.e. something that is not really a block device. |
| disk              | A disk                                                                       |
| partition         | A new physical partition                                                     |
| adopted-partition | An existing physical partition that is being adopted                         |
| raid-array        | A RAID array                                                                 |
| ab-volume         | An A/B volume                                                                |
| encrypted-volume  | An encrypted volume                                                          |
| verity-device     | A verity device                                                              |

## Referrer Description

This table lists all the different kinds of referrers that exist in the
configuration, along with their descriptions.

| Referrer kind      | Description                                                                                                                                                                                                  |
| ------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| none               | Represents a 'null referrer', i.e. an entity that does not refer to any block device.<br>E.g. Block devices that do not refer to any other block devices, such as disks, partitions, and adopted partitions. |
| raid-array         | A RAID array                                                                                                                                                                                                 |
| ab-volume          | An A/B volume                                                                                                                                                                                                |
| encrypted-volume   | An encrypted volume                                                                                                                                                                                          |
| verity-device      | A verity device                                                                                                                                                                                              |
| filesystem-new     | A new filesystem                                                                                                                                                                                             |
| filesystem-image   | A filesystem from an image                                                                                                                                                                                   |
| filesystem-esp     | An ESP/EFI filesystem                                                                                                                                                                                        |
| filesystem-adopted | An adopted filesystem                                                                                                                                                                                        |

## Reference Validity

This table contains the rules for valid references in the storage configuration.
The rows represent the different types of referrers that exists in the
configuration, and the columns represent the different types of block devices
that can be referenced.

A single cell in the table represents whether a referrer of a certain type can
reference a block device of a certain type.

| Referrer ╲ Device   | none | disk | partition | adopted-partition | raid-array | ab-volume | encrypted-volume | verity-device |
| ------------------- | ---- | ---- | --------- | ----------------- | ---------- | --------- | ---------------- | ------------- |
| none                | Yes  | No   | No        | No                | No         | No        | No               | No            |
| raid-array          | Yes  | No   | Yes       | No                | No         | No        | No               | No            |
| ab-volume           | Yes  | No   | Yes       | No                | Yes        | No        | Yes              | No            |
| encrypted-volume    | Yes  | No   | Yes       | No                | Yes        | No        | No               | No            |
| verity-device       | Yes  | No   | Yes       | No                | Yes        | Yes       | No               | No            |
| filesystem-new      | Yes  | No   | Yes       | No                | Yes        | Yes       | Yes              | Yes           |
| filesystem-image    | Yes  | No   | Yes       | No                | Yes        | Yes       | Yes              | Yes           |
| filesystem-esp      | Yes  | No   | Yes       | Yes               | Yes        | No        | No               | No            |
| filesystem-adopted  | Yes  | No   | No        | Yes               | No         | No        | No               | No            |

## Reference Count

A referrer may only reference a certain number of block devices. The table below
shows valid reference counts for each referrer type.

| Referrer type      | Min | Max |
| ------------------ | --- | --- |
| none               | 0   | 0   |
| raid-array         | 2   | ∞   |
| ab-volume          | 2   | 2   |
| encrypted-volume   | 1   | 1   |
| verity-device      | 2   | 2   |
| filesystem-new     | 0   | 1   |
| filesystem-image   | 1   | 1   |
| filesystem-esp     | 1   | 1   |
| filesystem-adopted | 1   | 1   |

## Reference Sharing

Most referrers claim exlusive access over their references. This table contains
the rules for sharing references in the storage configuration.

| Referrer type      | Valid sharing peers |
| ------------------ | ------------------- |
| none               | (none)              |
| raid-array         | (none)              |
| ab-volume          | (none)              |
| encrypted-volume   | (none)              |
| verity-device      | (none)              |
| filesystem-new     | (none)              |
| filesystem-image   | (none)              |
| filesystem-esp     | (none)              |
| filesystem-adopted | (none)              |

## Unique Field Value Constraints

Some block device types require that the value of a specific field be unique
across all block devices of that type.

| Device Kind       | Field Name |
| ----------------- | ---------- |
| disk              | device     |
| adopted-partition | matchLabel |
| adopted-partition | matchUuid  |
| raid-array        | name       |
| encrypted-volume  | deviceName |
| verity-device     | name       |

## Filesystem Block Device Requirements

Depending on the type of a filesystem, they may or may nor reference a backing
block device.

| Filesystem Type | Expects Block Device |
| --------------- | -------------------- |
| ext4            | Yes                  |
| xfs             | Yes                  |
| vfat            | Yes                  |
| ntfs            | Yes                  |
| swap            | Yes                  |
| tmpfs           | No                   |
| auto            | Yes                  |
| other           | Yes                  |

## Filesystem Source Requirements

Depending on the type of a filesystem, they may have different source types.

| Filesystem Type | Valid Source Type       |
| --------------- | ----------------------- |
| ext4            | new or adopted or image |
| xfs             | new or adopted or image |
| vfat            | new or adopted or image |
| ntfs            | new or adopted or image |
| swap            | new                     |
| tmpfs           | new                     |
| auto            | adopted                 |
| other           | image                   |

## Filesystem Mounting

Depending on the type of a filesystem, they may or may not have a mount point
configured.

| Filesystem Type | Mount Point   |
| --------------- | ------------- |
| ext4            | Optional      |
| xfs             | Optional      |
| vfat            | Optional      |
| ntfs            | Optional      |
| swap            | Not Supported |
| tmpfs           | Required      |
| auto            | Optional      |
| other           | Not Supported |

## Filesystem Verity Support

Depending on the type of a filesystem, they may or may not be used for verity.

| Filesystem Type | Supports Verity |
| --------------- | --------------- |
| ext4            | Yes             |
| xfs             | Yes             |
| vfat            | No              |
| ntfs            | No              |
| swap            | No              |
| tmpfs           | No              |
| auto            | No              |
| other           | No              |

## Homogeneous References

The following referrers require that all referenced devices are of the same type:

- raid-array
- ab-volume
- encrypted-volume
- verity-device

## Homogeneous Partition Types

The following referrers require that all underlying partitions are of the same type:

- raid-array
- ab-volume
- encrypted-volume
- filesystem-new
- filesystem-image
- filesystem-esp
- filesystem-adopted

## Homogeneous Partition Sizes

The following referrers require that all underlying partitions are of the same size:

- raid-array
- ab-volume

## Allowed Partition Types

Some referrers only support specific underlying partitions types.

| Referrer type      | Allowed partition types                                    |
| ------------------ | ---------------------------------------------------------- |
| none               | any                                                        |
| raid-array         | any                                                        |
| ab-volume          | any                                                        |
| encrypted-volume   | any type except 'esp' or 'root' or 'root-verity' or 'home' |
| verity-device      | 'root' or 'root-verity' or 'linux-generic'                 |
| filesystem-new     | any type except 'esp'                                      |
| filesystem-image   | any                                                        |
| filesystem-esp     | 'esp'                                                      |
| filesystem-adopted | any type except 'esp'                                      |

## Allowed RAID Levels

Some referrers may only refer to RAID arrays with certain RAID levels.

| Referrer type      | Allowed RAID levels           |
| ------------------ | ----------------------------- |
| none               | May not refer to a RAID array |
| raid-array         | May not refer to a RAID array |
| ab-volume          | any                           |
| encrypted-volume   | any                           |
| verity-device      | any                           |
| filesystem-new     | any                           |
| filesystem-image   | any                           |
| filesystem-esp     | 'raid1'                       |
| filesystem-adopted | May not refer to a RAID array |

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
| unknown       | Any path                         |

## Partition Type Matching Hash Partition

Partitions being used for verity need a matching partition for the hash data.

This table lists the expected hash partition for each partition type.
Types that are not listed are not valid for verity.

| Partition Type | Matching Hash Partition |
| -------------- | ----------------------- |
| root           | root-verity             |
| linux-generic  | linux-generic           |

