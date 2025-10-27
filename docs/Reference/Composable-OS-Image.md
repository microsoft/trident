---
sidebar_position: 6
title: COSI Spec
---

# Composable Operating System Image (COSI) Specification

## Revision Summary

| Revision            | Spec Date  |
| ------------------- | ---------- |
| [1.1](#revision-11) | 2025-05-08 |
| [1.0](#revision-10) | 2024-10-09 |

## Background

Trident is an image-based installer for Azure Linux. However, it does not deploy
simple disk images like other image-based installers, but instead, allows for
composability in the storage layout and structure. Because of that, it requires
a set of discrete images to install an Operating System.

Trident's original design was to consume multiple files, one per partition that
needed to be imaged. This design requires the user to manage multiple files in
all stages:

- Creation: Users need to create multiple files, one per partition. (Done
  through PRISM).
- Configuration: Users need to specify the paths to the multiple files in the
  Trident Host Configuration.
- Distribution: Users need to make sure all the images are available to Trident
  at the time of installation/update.

The multiple-files approach has manifested a few drawbacks, and not many
advantages. Some of the key drawbacks are:

- The need to manage multiple files.
- The risk of mixing files from different versions.
- The risk of missing files.
- The added verbosity of the Trident Host Configuration.
- The error-prone process of updating several image filenames and hashes in
  configuration files.

See more in the [Image Bundle Proposal
One-Pager](https://microsoft.sharepoint-df.com/:fl:/g/contentstorage/CSP_f0da4e64-56d1-4a82-845f-0fc5e98b83bb/EfVovKhKi89AjKbQX4bf0pgB_8S9SrV5qstrK6EriF541g?e=7AdTyk&nav=cz0lMkZjb250ZW50c3RvcmFnZSUyRkNTUF9mMGRhNGU2NC01NmQxLTRhODItODQ1Zi0wZmM1ZTk4YjgzYmImZD1iJTIxWkU3YThORldna3FFWHdfRjZZdUR1eU54N3hib3pXOUlqUXdma0Y0cnE3amo5MFgxdGhINFFhMHhscXdwMEJZcCZmPTAxM0ZKWk5WUFZOQzZLUVNVTFo1QUlaSldRTDZETjdVVVkmYz0lMkYmYT1Mb29wQXBwJnA9JTQwZmx1aWR4JTJGbG9vcC1wYWdlLWNvbnRhaW5lciZ4PSU3QiUyMnclMjIlM0ElMjJUMFJUVUh4dGFXTnliM052Wm5RdWMyaGhjbVZ3YjJsdWRDMWtaaTVqYjIxOFlpRmFSVGRoT0U1R1YyZHJjVVZZZDE5R05sbDFSSFY1VG5nM2VHSnZlbGM1U1dwUmQyWnJSalJ5Y1RkcWFqa3dXREYwYUVnMFVXRXdlR3h4ZDNBd1FsbHdmREF4TTBaS1drNVdTVFEzTTFvMk1sazFVelJhUWtsYVUwdERXVXhZUTBSTFRFbyUzRCUyMiUyQyUyMmklMjIlM0ElMjIxYjI1NThkMi05YzM5LTQ5NzgtOTgxZS0zMjUyOTgzMzY5ZTElMjIlN0Q%3D).

## Overview

This document describes the Composable OS Image (COSI) Specification. The COSI
format is a single file that contains all the images required to install a Linux
OS with Trident.

This document adheres to [RFC2119: Key words for use in RFCs to Indicate
  Requirement Levels](https://datatracker.ietf.org/doc/html/rfc2119).

## Goals

COSI should:

- Provide a one-file solution for users of PRISM and Trident.
- Be a portable and relatively trivial format.
- Contain all the images required to install a Linux OS
  with Trident.
- Contain enough metadata to inform Trident about the OS contained in the COSI
  file without adding extra verbosity to the Host Configuration.

## COSI File Format

The COSI file itself MUST be a simple uncompressed tarball with the extension
`.cosi`.

### Contents

The tarball MUST contain the following files:

- `metadata.json`: A JSON file that contains the metadata of the COSI file.
- Filesystem image files in the folder `images/`: The actual filesystem images
  that Trident will use to install the OS.

To allow for future extensions, the tarball MAY contain other files, but Trident
MUST ignore them. The tarball SHOULD NOT contain any extra files that will not
be used by Trident.

### Layout

The tarball MUST NOT have a common root directory. The metadata file MUST be at
the root of the tarball. If it were extracted with a standard `tar` invocation,
the metadata file would be placed in the current directory.

The metadata file SHOULD, be placed at the beginning of the tarball to allow for
quick access to the metadata without having to traverse the entire tarball.

### Partition Image Files

The partition image files are the actual images that Trident will use to install
the OS. These MUST be raw partition images.

The image files SHOULD be compressed. They SHOULD use ZSTD compression. Trident
only supports ZSTD-compressed images at the time of writing (2024-09-25), but
that could change in the future. Not using ZSTD-compressed images will result in
Trident failing to install the OS.

They MUST be located in a directory called `images/` inside the tarball. They
MAY be placed in subdirectories of `images/` to organize them. Trident MUST be
able to handle images in subdirectories.

### Metadata JSON File

The metadata file MUST be named `metadata.json` and MUST be at the root of the
tarball. The metadata file MUST be a valid JSON file.

#### Schema

##### Root Object

The metadata file MUST contain a JSON object with the following fields:

| Field        | Type                                   | Added in | Required        | Description                                      |
| ------------ | -------------------------------------- | -------- | --------------- | ------------------------------------------------ |
| `version`    | string `MAJOR.MINOR`                   | 1.0      | Yes (since 1.0) | The version of the metadata schema.              |
| `osArch`     | [OsArchitecture](#osarchitecture-enum) | 1.0      | Yes (since 1.0) | The architecture of the OS.                      |
| `osRelease`  | string                                 | 1.0      | Yes (since 1.0) | The contents of `/etc/os-release` verbatim.      |
| `images`     | [Filesystem](#filesystem-object)[]     | 1.0      | Yes (since 1.0) | Filesystem metadata.                             |
| `osPackages` | [OsPackage](#ospackage-object)[]       | 1.0      | Yes (since 1.1) | The list of packages installed in the OS.        |
| `bootloader` | [Bootloader](#bootloader-object)       | 1.1      | Yes (since 1.1) | Information about the bootloader used by the OS. |
| `id`         | UUID (string, case insensitive)        | 1.0      | No              | A unique identifier for the COSI file.           |

If the object contains other fields, readers MUST ignore them. A writer SHOULD
NOT add any other files to the object.

##### `Filesystem` Object

This object carries information about a filesystem and the partition it comes
from in a virtual disk.

| Field        | Type                                 | Added in | Required         | Description                               |
| ------------ | ------------------------------------ | -------- | ---------------- | ----------------------------------------- |
| `image`      | [ImageFile](#imagefile-object)       | 1.0      | Yes (since 1.0)  | Details of the image file in the tarball. |
| `mountPoint` | string                               | 1.0      | Yes (since 1.0)  | The mount point of the filesystem.        |
| `fsType`     | string                               | 1.0      | Yes (since 1.0)  | The filesystem's type. [1]                |
| `fsUuid`     | string                               | 1.0      | Yes (since 1.0)  | The UUID of the filesystem. [2]           |
| `partType`   | UUID (string, case insensitive)      | 1.0      | Yes (since 1.0)  | The GPT partition type. [3] [4] [5]       |
| `verity`     | [VerityConfig](#verityconfig-object) | 1.0      | Conditionally[6] | The verity metadata of the filesystem.    |

_Notes:_

- **[1]** It MUST use the name recognized by the kernel. For example, `ext4` for
    ext4 filesystems, `vfat` for FAT32 filesystems, etc.
- **[2]** It MUST be unique across all filesystems in the COSI tarball.
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

| Field      | Type                           | Added in | Required        | Description                                              |
| ---------- | ------------------------------ | -------- | --------------- | -------------------------------------------------------- |
| `image`    | [ImageFile](#imagefile-object) | 1.0      | Yes (since 1.0) | Details of the hash partition image file in the tarball. |
| `roothash` | string                         | 1.0      | Yes (since 1.0) | Verity root hash.                                        |

##### `ImageFile` Object

| Field              | Type   | Added in | Required        | Description                                                                               |
| ------------------ | ------ | -------- | --------------- | ----------------------------------------------------------------------------------------- |
| `path`             | string | 1.0      | Yes (since 1.0) | Absolute path of the compressed image file inside the tarball. MUST start with `images/`. |
| `compressedSize`   | number | 1.0      | Yes (since 1.0) | Size of the compressed image in bytes.                                                    |
| `uncompressedSize` | number | 1.0      | Yes (since 1.0) | Size of the raw uncompressed image in bytes.                                              |
| `sha384`           | string | 1.0      | Yes (since 1.1) | SHA-384 hash of the compressed hash image.                                                |

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
| `systemdBoot` | [`SystemDBoot`](#systemdboot-object)     | 1.1      | When `type` == `systemd-boot`[7] | systemd-boot configuration. |

_Notes:_

- **[7]** The `systemd-boot` field is required if the `type` field is set to
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

#### Samples

##### Simple Image

```json
{
    "version": "1.1",
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
            "partType": "root",
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
    ]
}
```

##### Verity Image with UKI

```json
{
    "version": "1.1",
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
    ]
}
```

## Changelog

### Revision 1.1

- Added `bootloader` field to the root object.
- Root field `osPackages` is now required.
- Field `sha384` in `ImageFile` object is now required.
- Fields `release` and `arch` in `OsPackage` object are now required.

### Revision 1.0

- Initial revision

## FAQ and Notes

**Why tar?**

- Tar is simple and ubiquitous. It is easy to create and extract tarballs on
  virtually any platform. There are native libraries for virtually every
  programming language to handle tarballs, including Rust and Go.
- Tar is a super simple tape format. It is just a stream of files with metadata
  at the beginning. This makes it easy to read and write.

**Why an uncompressed tarball?**

- The images SHOULD be compressed, and other than that the file should be pretty
  light-weight. Compressing the entire tarball does not yield a significant size
  reduction, if at all. This also allows us to read the metadata without having
  to extract the entire tarball.

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
  partition images. They are also not as portable as tarballs.
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
