# Trident Verity Test Image

This image is used for testing Trident on Mariner with `dm-verity` enabled. It
is based on the baremetal image and adds Trident as well as its dependencies. It
also adds openssh-server to allow for remote access. In addition to the [Trident
Test Image](../trident-testimage/README.md), this image is configured to use
dm-verity.

## Additional Prerequisites

- Artifacts
  - **Trident RPMs**: expected in `base/trident/*.rpm`. Can be downloaded with
    `make download-trident-rpms`

## Building

To build the base image and per-partition compressed images, run:

```bash
make trident-verity-testimage
```

It will populate the partition files as follows:

```text
build/trident-verity-testimage
├── esp.raw.zst
├── boot.raw.zst
├── root.raw.zst
├── root-hash.raw.zst
└── var.raw.zst
```
