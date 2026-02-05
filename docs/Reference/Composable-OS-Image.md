---
sidebar_position: 6
title: COSI Spec
---

# Composable Operating System Image (COSI) Specification

## Revision Summary

| Revision            | Spec Date  |
| ------------------- | ---------- |
| [1.2](#revision-12) | 2026-01-30 |
| [1.1](#revision-11) | 2025-05-08 |
| [1.0](#revision-10) | 2024-10-09 |

## Overview

This document describes the Composable OS Image (COSI) Specification. The COSI
format is a single file that contains all the images required to install a Linux
OS with Trident.

This document adheres to [RFC2119: Key words for use in RFCs to Indicate
  Requirement Levels](https://datatracker.ietf.org/doc/html/rfc2119).

## COSI File Format

The COSI file itself MUST be a simple uncompressed tar file with the extension
`.cosi`.

The tar file section of the COSI file MUST end with the standard tar
end-of-archive marker: two 512-byte blocks of zeroes, aligned to 512 bytes.

After the end-of-archive marker, the COSI file MAY contain additional data that
is not part of the tar archive. This additional data can be used for extensions
such as footers or checksums.

Readers MUST stop interpreting the COSI file as a tar file after the
end-of-archive marker.

### COSI as a Disk Image Format

COSI does not carry the full contents of a raw disk image, instead it only
contains the contents of the defined regions of a GPT-partitioned disk image.
This includes the primary GPT header and entries, partitions, but explicitly
EXCLUDES any unallocated space that is not part of any defined region.

The contents of any unallocated space outside of defined regions are NOT
included in the COSI file. However, the sizes and locations of these unallocated
spaces are preserved in the GPT data included in the COSI file.

### Contents

The tar file MUST contain the following files:

- `cosi-marker` file: An empty file named `cosi-marker` at the beginning of the
  tar file to identify the file as a COSI file.
- `metadata.json`: A JSON file that contains the metadata of the COSI file.
- Disk region images in the folder `images/`: ZSTD compressed images of the
  relevant regions of the source disk image. The region images in the COSI file
  MUST exist in the same physical order as they appear in the source disk image.

To allow for future extensions, the tar file MAY contain other files, but
Trident MUST ignore them. The tar file SHOULD NOT contain any extra files that
will not be used by Trident. Any extra files, if present, MUST exist after the
`cosi-marker` and `metadata.json` files.

### Tar file Layout

The tar file MUST NOT have a common root directory. The metadata file MUST be at
the root of the tar file. If it were extracted with a standard `tar` invocation,
the metadata file would be placed in the current directory. File names in the
tar metadata MUST NOT have any leading characters such as `./` or `/`.

The first entry of the tar file MUST be a regular file named `cosi-marker` of
size zero. This file serves as an identifier for the COSI format. See
[COSI Marker File](#cosi-marker-file) for more details.

The metadata file MUST be placed immediately after the `cosi-marker` file to
allow for quick discovery and access to the metadata without having to traverse
the entire tar file.

The disk region images SHOULD be placed right after the metadata file in the tar
file. The order of the image files in the tar file MUST match the original
PHYSICAL order of the regions in the source disk image.

### COSI Marker File

The COSI marker file MUST be named `cosi-marker` and MUST be the first entry in
the tar file. It MUST be a regular file of size zero bytes.

Because of the structure of a standard tar header, this makes the first 11 bytes
of the COSI file equal to `63 6f 73 69 2d 6d 61 72 6b 65 72`, which is the
binary representation of the ASCII string `cosi-marker`. Writers MUST use a tar
header format that produces this output. This includes using the standard USTAR,
GNU, and PAX (when no extended attributes are used) tar formats.

Readers MAY use this marker to quickly identify COSI files.

### Disk Region Images

The region images are compressed raw files containing the data inside each of
the relevant defined regions of the source disk image. The defined regions are:

- The primary GPT header and entries, along with the protective MBR.
- Each partition defined in the GPT partition entries.

For non-partition regions (such as the primary GPT header and entries and the
protective MBR), the corresponding region image MUST represent the entire region
and its uncompressed size MUST exactly match the size of that region on disk.

For partition regions, the logical size of the region image (its
`uncompressedSize` as recorded in the metadata) MAY be smaller than the full
partition size in cases where the writer shrunk the filesystem before
compressing the image. See [Filesystem Shrinking](#filesystem-shrinking) for
more details.

The images MUST be compressed using ZSTD compression.

They MUST exist in the tar file under the `images/` directory. They MAY be placed
in subdirectories of `images/` to organize them. Readers MUST be able to handle
images in subdirectories.

The physical order of the region images in the tar file MUST match the order
they appear in the source disk image, from the beginning of the disk to the end.

#### Filesystem Shrinking

For reduced size and increased disk-write efficiency, writers of COSI files
SHOULD shrink the filesystems before creating the images when:

- the filesystem type supports shrinking,
- the shrinking can be done safely, and
- the filesystem covers the entire partition, guaranteeing that no data outside
  the filesystem is lost.

Full coverage of the partition by the filesystem MUST be determined by the
writer based on the filesystem metadata and partition size. A filesystem is said
to cover the entire partition when the filesystem begins at the start of the
partition and:

- the filesystem's reported size exactly matches the partition size, OR
- the delta between the filesystem's reported size and the partition size is
  smaller than the filesystem's block size, meaning that the filesystem cannot
  grow to fill the partition.

Any resize operation MUST be done with standard tools for the filesystem type.

Readers MUST be able to handle filesystem images that have been shrunk and they
SHOULD resize the filesystem to fill the partition when writing the image
to disk.

To detect whether a filesystem has been shrunk, readers MUST compare the
`uncompressedSize` field of the `ImageFile` object with the size of the
partition as defined in the GPT partition entries.

- If the uncompressed image is smaller, the reader MUST assume that the
  filesystem has been shrunk.
- If the uncompressed image is equal to the partition size, the reader MUST
  assume that the image is intended to cover the full partition, its contents
  are raw and unshrunk, and MUST NOT attempt to resize it.
- If the uncompressed image is larger than the partition size, the reader MUST
  consider the COSI file invalid.

#### Compression

All region images in the COSI file MUST be compressed using ZSTD compression.
The compression level used is left to the writer's discretion.

Images must be compressed with different parameters as the writer sees fit.
However, writers MUST populate the `compression` field in the metadata with
parameters that will guarantee successful decompression of all images by
readers. See [`Compression Object`](#compression-object) for more details.

### GUID Partition Table (GPT) File

Starting from COSI version 1.2, the immediate next file after the metadata file
in the tar MUST be the ZSTD compressed binary image of the primary GPT header
and entries, along with the protective MBR. This image MUST be referenced as the
first entry in the `gptRegions` array of the `disk` object.

The image is defined to be everything from offset 0 of the original disk image
up to the end of the GPT entries, this includes:

- The protective MBR (LBA 0).
- The primary GPT header (at LBA 1).
- Any space between the primary GPT header and the GPT entries, if present.
- The GPT entries themselves. (generally starting at LBA 2).

Writers MUST determine the end of this region by reading the GPT header to find
the last GPT entry and calculating its end offset.

The uncompressed image MAY not be aligned to a logical block address (LBA)
boundary if the GPT entries do not end on an LBA boundary.

In standard GPTs with 128 entries of 128 bytes each, this image will include
LBAs 0 to 33 (inclusive), totaling 34 LBAs or 17 KiB.

The GPT bundled in COSI MUST have a valid header according to the
[UEFI Specification](https://uefi.org/specs/UEFI/2.10/05_GUID_Partition_Table_Format.html#guid-partition-table-gpt-disk-layout-1)
and MUST NOT contain overlapping partitions.

### Metadata JSON File

The metadata file MUST be named `metadata.json` and MUST be at the root of the
tar file. The metadata file MUST be a valid JSON file.

#### Schema

Any schema object MAY contain other fields not listed in the tables below.
Readers MUST ignore any fields not listed in the tables for future
compatibility.

##### Root Object

The metadata file MUST contain a JSON object with the following fields:

| Field         | Type                                   | Added in | Required        | Description                                      |
| ------------- | -------------------------------------- | -------- | --------------- | ------------------------------------------------ |
| `version`     | string `MAJOR.MINOR`                   | 1.0      | Yes (since 1.0) | The version of the metadata schema.              |
| `osArch`      | [OsArchitecture](#osarchitecture-enum) | 1.0      | Yes (since 1.0) | The architecture of the OS.                      |
| `osRelease`   | string                                 | 1.0      | Yes (since 1.0) | The contents of `/etc/os-release` verbatim.      |
| `images`      | [Filesystem](#filesystem-object)[]     | 1.0      | Yes (since 1.0) | Filesystem metadata.                             |
| `disk`        | [Disk](#disk-object)                   | 1.2      | Yes (since 1.2) | Original disk metadata.                          |
| `osPackages`  | [OsPackage](#ospackage-object)[]       | 1.0      | Yes (since 1.1) | The list of packages installed in the OS.        |
| `bootloader`  | [Bootloader](#bootloader-object)       | 1.1      | Yes (since 1.1) | Information about the bootloader used by the OS. |
| `id`          | UUID (string, case insensitive)        | 1.0      | No              | A unique identifier for the COSI file.           |
| `compression` | [Compression](#compression-object)     | 1.2      | Yes (since 1.2) | Compression metadata for the COSI file.          |

If the object contains other fields, readers MUST ignore them. A writer SHOULD
NOT add any other fields to the object.

##### `Filesystem` Object

This object carries information about a filesystem and the partition it comes
from in a virtual disk.

| Field        | Type                                 | Added in | Required         | Description                                |
| ------------ | ------------------------------------ | -------- | ---------------- | ------------------------------------------ |
| `image`      | [ImageFile](#imagefile-object)       | 1.0      | Yes (since 1.0)  | Details of the image file in the tar file. |
| `mountPoint` | string                               | 1.0      | Yes (since 1.0)  | The mount point of the filesystem.         |
| `fsType`     | string                               | 1.0      | Yes (since 1.0)  | The filesystem's type. [1]                 |
| `fsUuid`     | string                               | 1.0      | Yes (since 1.0)  | The UUID of the filesystem. [2]            |
| `partType`   | UUID (string, case insensitive)      | 1.0      | Yes (since 1.0)  | The GPT partition type. [3] [4] [5]        |
| `verity`     | [VerityConfig](#verityconfig-object) | 1.0      | Conditionally[6] | The verity metadata of the filesystem.     |

In COSI >= 1.2, all images referenced by the `image` field in the `Filesystem`
objects MUST have exactly one corresponding entry in the `gptRegions` array of
the `disk` object. Correspondence is determined by matching the `path` field of
the `ImageFile` objects. The `ImageFile` objects in both locations MUST be
exactly the same.

_Notes:_

- **[1]** It MUST use the name recognized by the kernel. For example, `ext4` for
    ext4 filesystems, `vfat` for FAT32 filesystems, etc.
- **[2]** It MUST be unique across all filesystems in the COSI tar file.
  Additionally, volumes in an A/B volume pair MUST have unique filesystem UUIDs.
- **[3]** It MUST be a UUID defined by the [Discoverable Partition Specification
    (DPS)](https://uapi-group.org/specifications/specs/discoverable_partitions_specification/)
    when the applicable type exists in the DPS. Other partition types MAY be
    used for types not defined in DPS (e.g. Windows partitions).
- **[4]** The EFI Sytem Partition (ESP) MUST be identified with the UUID
    established by the DPS: `c12a7328-f81f-11d2-ba4b-00a0c93ec93b`.
- **[5]** Should default to `0fc63daf-8483-4772-8e79-3d69d8477de4` (Generic
    Linux Data) if the partition type cannot be determined.
- **[6]** The `verity` field MUST be specified if the OS is configured to open this
    filesystem with `dm-verity`. Otherwise, it MUST be omitted OR set to `null`.

##### `VerityConfig` Object

The `VerityConfig` object contains information required to set up a verity
device on top of a data device.

| Field      | Type                           | Added in | Required        | Description                                               |
| ---------- | ------------------------------ | -------- | --------------- | --------------------------------------------------------- |
| `image`    | [ImageFile](#imagefile-object) | 1.0      | Yes (since 1.0) | Details of the hash partition image file in the tar file. |
| `roothash` | string                         | 1.0      | Yes (since 1.0) | Verity root hash.                                         |

##### `ImageFile` Object

| Field              | Type   | Added in | Required        | Description                                                                       |
| ------------------ | ------ | -------- | --------------- | --------------------------------------------------------------------------------- |
| `path`             | string | 1.0      | Yes (since 1.0) | Path of the compressed image file inside the tar file. MUST start with `images/`. |
| `compressedSize`   | number | 1.0      | Yes (since 1.0) | Size of the compressed image in bytes.                                            |
| `uncompressedSize` | number | 1.0      | Yes (since 1.0) | Size of the raw uncompressed image in bytes.                                      |
| `sha384`           | string | 1.0      | Yes (since 1.1) | SHA-384 hash of the compressed image.                                             |

##### `Disk` Object

The `disk` field holds information about the original disk layout of the image
this COSI file was sourced from.

| Field        | Type                                     | Added in | Required             | Description                                                        |
| ------------ | ---------------------------------------- | -------- | -------------------- | ------------------------------------------------------------------ |
| `size`       | number                                   | 1.2      | Yes (since 1.2)      | Size of the original disk in bytes.                                |
| `type`       | [DiskType](#disktype-enum)               | 1.2      | Yes (since 1.2)      | Partitioning type of the original disk.                            |
| `lbaSize`    | number                                   | 1.2      | Yes (since 1.2)      | The size of a logical block address (LBA) in bytes. Generally 512. |
| `gptRegions` | [GptDiskRegion](#gptdiskregion-object)[] | 1.2      | When `type` == `gpt` | Regions in the GPT disk.                                           |

The order of the `gptRegions` array MUST match the physical order of the regions
in the original disk image, from the beginning of the disk to the end.

In COSI `>=1.2` when `type` == `gpt`, `gptRegions` MUST contain exactly one
primary-gpt entry and one partition entry for each partition present both in the
GPT and in the tar file, ordered by increasing start LBA (with primary-gpt
first).

The field `size` MUST be a multiple of `lbaSize`.

##### `DiskType` Enum

The partitioning table type. Currently, only `gpt` is supported.

| Value | Description                                          |
| ----- | ---------------------------------------------------- |
| `gpt` | The disk uses the GUID Partition Table (GPT) scheme. |

##### `GptDiskRegion` Object

This object holds information about a specific region of the original disk
image.

| Field    | Type                           | Added in | Required                   | Description                                      |
| -------- | ------------------------------ | -------- | -------------------------- | ------------------------------------------------ |
| `image`  | [ImageFile](#imagefile-object) | 1.2      | Yes (since 1.2)            | Details of the image file in the tar file.       |
| `type`   | [RegionType](#regiontype-enum) | 1.2      | Yes (since 1.2)            | The type of region this image represents.        |
| `number` | number                         | 1.2      | When `type` == `partition` | The partition's GPT entry index (1-based index). |

##### `RegionType` Enum

The type of region in the original disk image.

| Value         | Description                                                                                              |
| ------------- | -------------------------------------------------------------------------------------------------------- |
| `primary-gpt` | Everything from offset 0 to the end of the primary GPT header and entries, including the protective MBR. |
| `partition`   | A partition as defined in the GPT partition entries.                                                     |

##### `OsArchitecture` Enum

The `osArch` field in the root object MUST be a string that represents the
architecture of the OS. The following table lists the valid values for the
`osArch` field.

| Value    | Description                         |
| -------- | ----------------------------------- |
| `x86_64` | AMD64 or Intel 64-bit architecture. |
| `arm64`  | ARM 64-bit architecture.            |

_Note:_ The `osArch` field uses the names reported by `uname -m` for consistency.
The `osArch` field is case-insensitive.

##### `OsPackage` Object

The `osPackages` field in the root object MUST contain an array of `OsPackage`
objects. Each object represents a package installed in the OS.

| Field     | Type   | Added in | Required        | Description                           |
| --------- | ------ | -------- | --------------- | ------------------------------------- |
| `name`    | string | 1.0      | Yes (since 1.0) | The name of the package.              |
| `version` | string | 1.0      | Yes (since 1.0) | The version of the package installed. |
| `release` | string | 1.0      | Yes (since 1.1) | The release of the package.           |
| `arch`    | string | 1.0      | Yes (since 1.1) | The architecture of the package.      |

A suggested way to obtain this information is by running:

```bash
rpm -qa --queryformat "%{NAME} %{VERSION} %{RELEASE} %{ARCH}\n"
```

##### `Bootloader` Object

| Field         | Type                                     | Added in | Required                         | Description                 |
| ------------- | ---------------------------------------- | -------- | -------------------------------- | --------------------------- |
| `type`        | [`BootloaderType`](#bootloadertype-enum) | 1.1      | Yes (since 1.1)                  | The type of the bootloader. |
| `systemdBoot` | [`SystemDBoot`](#systemdboot-object)     | 1.1      | When `type` == `systemd-boot`[1] | systemd-boot configuration. |

_Notes:_

- **[1]** The `systemd-boot` field is required if the `type` field is set to
    `systemd-boot`. It MUST be omitted OR set to `null` if the `type`
    field is set to any other value.

##### `BootloaderType` Enum

A string that represents the primary bootloader used in the contained OS. These
are the valid values for the `type` field in the `bootloader` object:

| Value          | Description                                         |
| -------------- | --------------------------------------------------- |
| `systemd-boot` | The system is using systemd-boot as the bootloader. |
| `grub`         | The system is using GRUB as the bootloader.         |

##### `SystemDBoot` Object

This object contains metadata about how systemd-boot is configured in the OS.

| Field     | Type                                             | Added in | Required        | Description                                                                          |
| --------- | ------------------------------------------------ | -------- | --------------- | ------------------------------------------------------------------------------------ |
| `entries` | [`SystemDBootEntry`](#systemdbootentry-object)[] | 1.1      | Yes (since 1.1) | The contents of the `loader/entries/*.conf` files in the systemd-boot EFI partition. |

##### `SystemDBootEntry` Object

This object contains metadata about a specific systemd-boot entry.

| Field     | Type                                                 | Added in | Required        | Description                                            |
| --------- | ---------------------------------------------------- | -------- | --------------- | ------------------------------------------------------ |
| `type`    | [`SystemDBootEntryType`](#systemdbootentrytype-enum) | 1.1      | Yes (since 1.1) | The type of the entry.                                 |
| `path`    | string                                               | 1.1      | Yes (since 1.1) | Absolute path (from the root FS) to the UKI or config. |
| `cmdline` | string                                               | 1.1      | Yes (since 1.1) | The kernel command line.                               |
| `kernel`  | string                                               | 1.1      | Yes (since 1.1) | Kernel release as a string.                            |

##### `SystemDBootEntryType` Enum

A string that represents the type of the systemd-boot entry.

| Value            | Description                                                        |
| ---------------- | ------------------------------------------------------------------ |
| `uki-standalone` | The entry is a bare UKI file in the ESP.                           |
| `uki-config`     | The entry is a config file with a UKI.                             |
| `config`         | The entry is a config file with a kernel, initrd and command line. |

##### `Compression` Object

This object contains metadata about the compression settings used to produce the
COSI file.

| Field          | Type   | Added in | Required        | Description                                       |
| -------------- | ------ | -------- | --------------- | ------------------------------------------------- |
| `maxWindowLog` | number | 1.2      | Yes (since 1.2) | The max zstd `windowLog`used for compression. [1] |

_Notes:_

- **[1]** The `windowLog` is the "Maximum allowed back-reference distance,
  expressed as power of 2" (See: [zstd
  manual](https://facebook.github.io/zstd/zstd_manual.html)) used during
  compression of a file. The writer MUST populate this field with the maximum
  value used across all images in the COSI file to guarantee successful
  decompression of all images by readers.

#### Samples

##### Simple Image

Note: these are not complete samples and they contain comments for explanation,
making them invalid JSON. They are provided for illustration purposes only.

```json
{
    "version": "1.2",
    "images": [
        {
            "image": {
                "path": "images/esp.rawzst",
                "compressedSize": 839345,
                "uncompressedSize": 8388608,
                "sha384": "2decc64a828dbbb76779731cd4afd3b86cc4ad0af06f4afe594e72e62e33e520a6649719fe43f09f11d518e485eae0db"
            },
            "mountPoint": "/boot/efi",
            "fsType": "vfat",
            "fsUuid": "C3D4-250D",
            "partType": "c12a7328-f81f-11d2-ba4b-00a0c93ec93b", // <-- ESP DPS GUID
            "verity": null
        },
        {
            "image": {
                "path": "images/root.rawzst",
                "compressedSize": 192874245,
                "uncompressedSize": 899494400,
                "sha384": "98ea4adbbb8ce0220d109d53d65825bd5a565248e4af3a9346d088918e7856ac2c42e13461cac67dbf3711ff69695ec3"
            },
            "mountPoint": "/",
            "fsType": "ext4",
            "fsUuid": "88d2fa9b-7a32-450a-a9f8-aa9c3de79298",
            "partType": "4f68bce3-e8cd-4db1-96e7-fbcaf984b709", // <-- Root amd64/x86_64 DPS GUID
            "verity": null
        }
    ],
    "osRelease": "NAME=\"Microsoft Azure Linux\"\nVERSION=\"3.0.20240824\"\nID=azurelinux\nVERSION_ID=\"3.0\"\nPRETTY_NAME=\"Microsoft Azure Linux 3.0\"\nANSI_COLOR=\"1;34\"\nHOME_URL=\"https://aka.ms/azurelinux\"\nBUG_REPORT_URL=\"https://aka.ms/azurelinux\"\nSUPPORT_URL=\"https://aka.ms/azurelinux\"\n",
    "bootloader": {
        "type": "grub"
    },
    "osPackages": [
        {
            "name": "bash",
            "version": "5.1.8",
            "release": "1.azl3",
            "arch": "x86_64"
        },
        {
            "name": "coreutils",
            "version": "8.32",
            "release": "1.azl3",
            "arch": "x86_64"
        },
        {
            "name": "systemd",
            "version": "255",
            "release": "20.azl3",
            "arch": "x86_64"
        },
        // More packages...
    ],
    "disk": {
        "size": 1073741824,
        "type": "gpt",
        "lbaSize": 512,
        "gptRegions": [
            {
                "image": {
                    "path": "images/primary-gpt.rawzst",
                    "compressedSize": 16384,
                    "uncompressedSize": 32768,
                    "sha384": "a3f5c6e2b4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b1c2d3e4f5a6b7c8d9e0f1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7"
                },
                "type": "primary-gpt"
            },
            {
                "image": {
                    "path": "images/esp.rawzst",
                    "compressedSize": 839345,
                    "uncompressedSize": 8388608,
                    "sha384": "2decc64a828dbbb76779731cd4afd3b86cc4ad0af06f4afe594e72e62e33e520a6649719fe43f09f11d518e485eae0db"
                },
                "type": "partition",
                "number": 1
            },
            // More regions...
        ]
    },
    "compression": {
        "windowSize": 30 // <-- Non-default 1 GiB window size
    }
}
```

##### Verity Image with UKI

```json
{
    "version": "1.2",
    "images": [
        {
            "image": {
                "path": "images/root.rawzst",
                "compressedSize": 192874245,
                "uncompressedSize": 899494400,
                "sha384": "98ea4adbbb8ce0220d109d53d65825bd5a565248e4af3a9346d088918e7856ac2c42e13461cac67dbf3711ff69695ec3"
            },
            "mountPoint": "/",
            "fsType": "ext4",
            "fsUuid": "88d2fa9b-7a32-450a-a9f8-aa9c3de79298",
            "partType": "4f68bce3-e8cd-4db1-96e7-fbcaf984b709", // <-- Root amd64/x86_64 DPS GUID
            "verity": {
                "image": {
                    "path": "images/root-verity.rawzst",
                    "compressedSize": 26214400,
                    "uncompressedSize": 524288000,
                    "sha384": "51356c53fbdd5c196395ccd389116f2e7769443cb4e945bc9b6bc3c805cf857c375df010469f8f45ef0c5b07456b023d"
                },
                "roothash": "646c82fa4c3f97e6cddc3996315c7f04b2beb721fb24fa38835136492a84eb19"
            }
        },
        // More images...
    ],
    "osRelease": "NAME=\"Microsoft Azure Linux\"\nVERSION=\"3.0.20240824\"\nID=azurelinux\nVERSION_ID=\"3.0\"\nPRETTY_NAME=\"Microsoft Azure Linux 3.0\"\nANSI_COLOR=\"1;34\"\nHOME_URL=\"https://aka.ms/azurelinux\"\nBUG_REPORT_URL=\"https://aka.ms/azurelinux\"\nSUPPORT_URL=\"https://aka.ms/azurelinux\"\n",
    "bootloader": {
        "type": "systemd-boot",
        "systemdBoot": {
            "entries": [
                {
                    "type": "uki-standalone",
                    "path": "/boot/efi/EFI/Linux/azurelinux-uki.efi",
                    "cmdline": "root=/dev/disk/by-partuuid/88d2fa9b-7a32-450a-a9f8-aa9c3de79298 ro",
                    "kernel": "6.6.78.1-3.azl3"
                }
            ]
        }
    },
    "osPackages": [
        {
            "name": "systemd",
            "version": "255",
            "release": "20.azl3",
            "arch": "x86_64"
        },
        // More packages...
    ],
    "disk": {
        "size": 1073741824,
        "type": "gpt",
        "lbaSize": 512,
        "gptRegions": [
            {
                "image": {
                    "path": "images/primary-gpt.rawzst",
                    "compressedSize": 16384,
                    "uncompressedSize": 32768,
                    "sha384": "a3f5c6e2b4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b1c2d3e4f5a6b7c8d9e0f1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7"
                },
                "type": "primary-gpt"
            },
            // More regions...
        ]
    }
}
```

## Changelog

### Revision 1.2

- Added `disk` field to the root object.
- Added `compression` field to the root object.
- COSI now ships the GPT data as a binary blob.
- Added `cosi-marker` file as the first entry in the tar file.

### Revision 1.1

- Added `bootloader` field to the root object.
- Root field `osPackages` is now required.
- Field `sha384` in `ImageFile` object is now required.
- Fields `release` and `arch` in `OsPackage` object are now required.

### Revision 1.0

- Initial revision

## FAQ and Notes

**Why tar?**

- Tar is simple and ubiquitous. It is easy to create and extract tar files on
  virtually any platform. There are native libraries for virtually every
  programming language to handle tar files, including Rust and Go.
- Tar is a super simple tape format. It is just a stream of files with metadata
  at the beginning. This makes it easy to read and write.

**Why an uncompressed tar file?**

- The images SHOULD be compressed, and other than that the file should be pretty
  light-weight. Compressing the entire tar file does not yield a significant size
  reduction, if at all. This also allows us to read the metadata without having
  to extract the entire tar file.

**Why not ZIP?**

- ZIP is more complex than tar. It has more features, notably an index at the
  end of the file. However, to compute the hash of the file, we need to read it
  through, anyway, so we can index the file as we read it. Even in cases where
  we don't need to compute the hash, to take full advantage of the index, we
  would need to implement our own ZIP reader.
- ZSTD support in ZIP is not very
  widespread.

**Why not use a custom format?**

- Making a custom format MAY help us achieve greater performance is some edge
  cases, specifically network streaming. However, the complexity of creating and
  maintaining a custom format outweighs the benefits. Tar is simple and good
  enough for our needs.

**Why not use VHD or VHDX?**

- VHD and VHDX are complex formats that are not designed for our use case. They
  are designed to be used as virtual disks, not as a simple container for
  partition images. They are also not as portable as tar files.
- They do not have a standard way to store metadata. The spec does include some
  empty space reserved for future expansion, but using it would require us to
  implement our own fork of the VHD/VHDX spec.

  **What about a VHD+Metadata?**

  - Putting the metadata in a separate file would defeat the purpose of having a
    single file.

**What other formats were considered?**

- We considered using a custom format, but the complexity of creating and
  maintaining a custom format outweighs the benefits.
- SquashFS was considered, but it would only change the container around the
  filesystems images. When considering only the container, there was no real
  practical benefit to using SquashFS over Tar.
