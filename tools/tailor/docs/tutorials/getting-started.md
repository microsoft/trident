# Getting started

In this tutorial you will create one standalone image definition and render the Image Customizer invocation without building an image.

## 1. Install tailor

Install from source (see [Installation](../installation.md) for the from-monorepo option):

```bash
cargo install --path crates/tailor
```

Check it:

```bash
tailor --version
```

Expected shape:

```text
tailor <version>+<commit>.<date>
```

## 2. Scaffold a standalone image

Create a new empty directory, then run:

```bash
mkdir solo-demo
cd solo-demo
tailor init solo simple
```

Expected output includes:

```text
created .../solo-demo/image.yaml
Scaffolded standalone image `solo`. Try: tailor validate
```

The `simple` template creates only `./image.yaml`; there is no `tailor.yaml`. In standalone mode tailor uses its built-in default Image Customizer toolchain: `mcr.microsoft.com/azurelinux/imagecustomizer:latest`.

## 3. Read the image definition

Open `image.yaml`:

```yaml
name: solo
outputs:
  - format: cosi
base:
  azureLinux:
    version: "3.0"
    variant: minimal-os
config:
  previewFeatures:
    - input-image-oci
  os:
    hostname: solo
    bootloader:
      resetType: hard-reset
    packages:
      install:
        - openssh-server
    services:
      enable:
        - sshd
  storage:
    bootType: efi
    disks:
      - partitionTableType: gpt
        maxSize: 4G
        partitions:
          - { id: esp, type: esp, size: 8M }
          - { id: rootfs, size: grow }
    filesystems:
      - { deviceId: esp, type: fat32, mountPoint: { path: /boot/efi, options: "umask=0077" } }
      - { deviceId: rootfs, type: ext4, mountPoint: { path: / } }
```

This is the complete scaffold, with the storage lists shown in compact YAML. The
`input-image-oci` opt-in lets IC fetch the Azure Linux base; the growing root partition and
`hard-reset` bootloader setting support package installation. Top-level keys are tailor's;
`config:` passes through to IC.

## 4. Validate

```bash
tailor validate
```

Expected shape:

```text
✓ solo                         1 cell(s) valid
```

## 5. Dry-run the build

```bash
tailor build --dry-run
```

Expected shape:

```text
1 cell(s) (dry-run)

docker run \
  ...
  mcr.microsoft.com/azurelinux/imagecustomizer:latest \
  customize \
  --config-file ... \
  ...
  --output-image-format cosi \
  ...
```

`--dry-run` prints the container/IC invocation without starting a container or contacting an
engine. The host paths depend on your directory.

## 6. Build the image (optional)

With a reachable Docker daemon and registry access, run:

```bash
tailor build
```

The generated output is `artifacts/solo_amd64_cosi.cosi`. To check it:

```bash
test -f artifacts/solo_amd64_cosi.cosi
```

## Next step

Learn matrix builds in [Your first matrix](your-first-matrix.md).
