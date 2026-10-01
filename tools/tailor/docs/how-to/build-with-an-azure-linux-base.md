# Build an image from an Azure Linux base

This walks through a real `tailor build` using an `azureLinux` (MCR) base — the common case where you
don't supply a base image file yourself.

## Prerequisites

- **Docker** (or Podman — see [Select a container engine](select-a-container-engine.md)). A real
  build runs Image Customizer in a privileged container.

## The manifest

```yaml
# image.yaml
name: appliance
base:
  azureLinux:
    version: "3.0"
    variant: minimal-os     # the smallest Azure Linux variant
outputs:
  - format: cosi
config:
  # `azureLinux`/`oci` bases are downloaded by IC's OCI input, which is a preview feature. tailor
  # never edits your IC config, so enable it here yourself:
  previewFeatures:
    - input-image-oci
  os:
    hostname: appliance
    bootloader:
      resetType: hard-reset
  storage:
    bootType: efi
    disks:
      - partitionTableType: gpt
        maxSize: 4G
        partitions:
          - { id: esp, type: esp, size: 8M }
          - { id: rootfs, size: grow }
    filesystems:
      - deviceId: esp
        type: fat32
        mountPoint: { path: /boot/efi, options: "umask=0077" }
      - deviceId: rootfs
        type: ext4
        mountPoint: { path: / }
```

> **The `input-image-oci` preview feature is required** for any `oci`/`azureLinux` base. Without it,
> Image Customizer fails with `preview feature 'input-image-oci' required to specify OCI input image`.

## Build it

```bash
tailor build
```

```text
   Toolchain mcr.microsoft.com/azurelinux/imagecustomizer:latest
    Building 1 cell(s) selected, 1 to build
 Customizing appliance_amd64_cosi  (azureLinux 3.0/minimal-os)
       Built artifacts/appliance_amd64_cosi.cosi (130.0 MiB)
    Finished 1 artifact(s) in 38.4s
```

Pass `-vv` to also stream the Image Customizer logs.

## What tailor handles for you

- **Digest pinning.** tailor resolves the base to a digest and passes IC a reproducible
  `--image oci:<repo>@sha256:…`. Run `tailor lock` to record it in `tailor.lock`.
- **Image cache.** `oci`/`azureLinux` bases need a cache directory; tailor defaults
  `runtime.imageCacheDir` to `<workspace>/.tailor/cache`.
- **Ownership.** IC writes outputs as root; the sudo-free janitor normalizes them (and the cache) back
  to you, so artifacts are yours to read and delete. tailor defaults `runtime.janitorImage` to
  `mcr.microsoft.com/azurelinux/base/core:3.0`.

Set any of these explicitly under `runtime:` in `tailor.yaml` to override the defaults.
