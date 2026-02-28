# Disk Streaming

Disk streaming is a servicing type offered by Trident to bootstrap machines by
applying a remote OS image to disk. It is designed as a more performant
alternative to flashing traditional disk image formats (such as VHD or VHDX) by
leveraging the [COSI](./COSI.md) format. Because COSI excludes unallocated
space, shrinks filesystems, and compresses partition data, disk streaming
requires significantly less bandwidth and time compared to writing a full raw
disk image.

The disk image must be in [COSI](./COSI.md) format. COSI images contain the
partition data along with metadata that describes the disk layout, partition
hashes, and compression information. This metadata allows Trident to derive a
complete deployment plan from the image alone without a separate Host
Configuration file.

:::note

Only COSI v1.2+ images are supported for disk streaming, as they contain the
necessary metadata for this servicing type. This version of COSI can be produced
with Image Customizer ≥ v1.2.

:::

## Disk Streaming vs. Install

Image streaming is used as the underlying transfer mechanism in multiple
servicing types, but the **stream-disk** and **install** services represent two
fundamentally different approaches to provisioning a host.

### Install

An install is driven by a
[Host Configuration](../Reference/Host-Configuration/API-Reference/HostConfiguration.md)
file, which is the ultimate authority on how the system should be laid out. The
Host Configuration gives the operator full control over disk partitioning,
filesystem types, A/B volume pairs, RAID arrays, encryption, verity, boot
configuration, extensions, and more. Because the operator defines every aspect
of the target state, install supports the widest range of Trident features and
is the recommended approach for production deployments.

### Stream Disk

The `stream-disk` service (available via the [gRPC server](./gRPC-Server.md))
takes the opposite approach: it derives the entire disk layout from the image
itself. Instead of requiring a Host Configuration, `stream-disk` reads the
metadata embedded in a COSI v1.2+ image and automatically determines how to
partition and write to the disk:

1. The image is fetched from the provided URL.
2. Disk layout and partition configuration are extracted from the image metadata.
3. Trident selects the smallest disk that fits the image.
4. The image is streamed to the selected disk using the standard streaming
   pipeline.

This makes `stream-disk` simpler to invoke — only a URL and an optional hash
are needed — but it trades flexibility for convenience. Features that require
explicit operator decisions in the Host Configuration (such as custom partition
sizes, RAID, encryption, or multi-disk layouts) are not available through
`stream-disk`. It is best suited for scenarios where the image is
self-describing and no additional host-specific configuration is needed.

Disk streaming leverages the
[image streaming pipeline](./Image-Streaming-Pipeline.md) to transfer data
efficiently — with low memory usage, no temporary storage, built-in integrity
verification, and network resilience.

## How It Works

When a `stream-disk` request is received, Trident performs the following steps:

### 1. Fetch and Validate the COSI Image

Trident downloads the [COSI](./COSI.md) image from the provided URL and reads
its metadata. If a hash was provided in the request, Trident verifies the
metadata integrity before proceeding.

### 2. Derive the Disk Layout

Unlike an install, where the operator defines the disk layout in a Host
Configuration, disk streaming extracts the partition table and disk geometry
directly from the COSI metadata and applies it as-is. Trident selects the
smallest available disk that can accommodate the image and writes the GPT
partition table exactly as described in the metadata, without modifications.

### 3. Stream Partition Images

Each partition image contained in the COSI file is streamed to its
corresponding block device using the
[image streaming pipeline](./Image-Streaming-Pipeline.md). The images are
decompressed, hashed, and written in a single pass.

### 4. Minimal System Modification

In contrast to an install — where Trident may configure bootloader entries,
set up A/B volume pairs, apply verity, run subsystems, and perform other
host-specific adjustments — disk streaming explicitly avoids making any extra
modifications to the system beyond writing the partition data. The goal is to
reproduce the disk contents from the COSI image as faithfully as possible,
leaving the system in the exact state described by the image.
