# Composable OS Image (COSI)

The Composable OS Image (COSI) is the image format used by Trident for OS
deployment. A COSI file is a single archive that packages all the partition
images needed to install a Linux OS, along with metadata that describes the
disk layout, filesystem properties, and integrity hashes.

## Why COSI?

Traditional disk image formats (such as raw or QCOW2) carry the full contents
of a disk, including large stretches of empty or unallocated space. This makes
them expensive to store, transfer, and write. COSI was designed specifically to
address these challenges:

- **No wasted space** — COSI only includes the defined regions of the disk
  (GPT header, partitions). Unallocated space between partitions is excluded
  from the archive, significantly reducing file size.
- **Filesystem shrinking** — before creating the archive, filesystems can be
  shrunk to remove unused blocks. On the target host, Trident expands them back
  to fill the partition. This further reduces the amount of data that needs to
  be transferred and written.
- **ZSTD compression** — all partition images inside the COSI file are
  compressed with [ZSTD](https://facebook.github.io/zstd/), providing high
  compression ratios with fast decompression.
- **Sparse reads** — Trident can read individual partitions directly from a
  remote server using HTTP Range requests without downloading the entire file.
  See the [Image Streaming Pipeline](./Image-Streaming-Pipeline.md) for
  details.
- **Integrity verification** — the metadata includes a SHA-384 hash for every
  partition image. Trident verifies each hash as data is streamed to disk,
  catching corruption immediately.
- **Self-describing** — the metadata captures the full disk layout (partition
  table, partition types, sizes, and filesystem details), the OS architecture,
  the installed package list, and the bootloader type. This allows Trident to
  derive a complete deployment plan from the image alone when using
  [disk streaming](./Disk-Streaming.md).

## What Is Inside a COSI File?

At a high level, a COSI file is an uncompressed tar archive (`.cosi` extension)
containing:

1. **`cosi-marker`** — an empty file at the very beginning of the archive that
   identifies it as a COSI file.
2. **`metadata.json`** — a JSON document describing the disk layout, filesystem
   properties, compression parameters, bootloader type, OS architecture, and
   the list of partition images with their offsets, sizes, and hashes.
3. **`images/`** — ZSTD-compressed images of each disk region (GPT header and
   partitions), stored in the same physical order they appear on the source
   disk.

The use of a standard tar container means COSI files can be inspected with
common tools, while the structured metadata enables Trident to efficiently
locate and stream only the partitions it needs.

## Creating COSI Files

The recommended tool for creating COSI files is
[Image Customizer](https://microsoft.github.io/azure-linux-image-tools/imagecustomizer/README.html),
part of the
[azure-linux-image-tools](https://github.com/microsoft/azure-linux-image-tools)
project. Image Customizer takes a base Azure Linux image and a configuration
file, customizes the OS (packages, partitions, bootloader, verity, etc.), and
can produce a COSI file as its output format.

While Image Customizer is the official and recommended path, the COSI format
is fully specified and open. The complete specification is available in the
[COSI Specification](../Reference/Composable-OS-Image.md) reference document,
making it possible for other tools to read or produce COSI files.

## How Trident Uses COSI

For a detailed walkthrough of how Trident reads the COSI metadata and streams
partition images to disk, see
[How Trident Consumes COSI](./How-Trident-Consumes-COSI.md).
