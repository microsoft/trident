
# Set Up Root Verity

## Goals

Configuring verity for the root (`/`) partition offers good protection against modification of the installed operating system. Applying verity to root does make configuring system processes and services more difficult.

> Note: Another option is using verity for the [usr (`/usr`) partition](./Usr-Verity.md) which offers good protection for executables, while allowing configuration.

This goal of this document is to enable you to create a [COSI](../Refernce/COSI.md) file that sets up root-verity.

## Prerequisites

1. Ensure that [oras](https://oras.land/docs/installation/) is installed.
2. Ensure [Image Customizer container](https://microsoft.github.io/azure-linux-image-tools/imagecustomizer/quick-start/quick-start.html) is accessible.

## Instructions

### Step 1: Download the minimal base image

Pull [minimal-os](../Reference/Glossary.md#minimal-os) as a base image from MCR by running:

``` bash
mkdir -p $HOME/staging
pushd $HOME/staging
oras pull mcr.microsoft.com/azurelinux/3.0/image/minimal-os:latest
popd
```

### Step 2: Get Trident RPMs

Build the Trident RPMs by running:

``` bash
make bin/trident-rpms.tar.gz
```

After running this make command, the RPMs will be built and packaged into `bin/trident-rpms.tar.gz` and unpacked into `bin/RPMS/x86_64`:

``` bash
$ ls bin/RPMS/x86_64/
trident-0.3.DATESTRING-dev.COMMITHASH.azl3.x86_64.rpm
trident-install-service-0.3.DATESTRING-dev.COMMITHASH.azl3.x86_64.rpm
trident-provisioning-0.3.DATESTRING-dev.COMMITHASH.azl3.x86_64.rpm
trident-service-0.3.DATESTRING-dev.COMMITHASH.azl3.x86_64.rpm
trident-static-pcrlock-files-0.3.DATESTRING-dev.COMMITHASH.azl3.x86_64.rpm
trident-update-poll-0.3.DATESTRING-dev.COMMITHASH.azl3.x86_64.rpm
```

Copy RPMs to staging folder:

``` bash
cp -r bin/RPMS $HOME/staging
```

### Step 3: Create Image Customizer Configuration

To create a root-verity volume, there are a few Image Customization configuration sections that are important.

In addition to the typical `root` parition definition, a `root-hash` partition is needed like this:

``` yaml
storage:
  disks:
  - partitionTableType: gpt
    partitions:
    - label: root-hash
      id: verityhash
      size: 128M
```

Image Customizer needs some information to coordinate the `root` and `root-hash` partitions as part of a verity volume:

``` yaml
verity:
  - id: rootverity
    name: root
    dataDeviceId: root
    hashDeviceId: verityhash
    dataDeviceMountIdType: part-label
    hashDeviceMountIdType: part-label
```

Putting that all together and following the Image Customizer [documentation](https://microsoft.github.io/azure-linux-image-tools/imagecustomizer/README.html), the full configuration `$HOME/staging/ic-config.yaml` can look like this:

``` yaml
storage:
  disks:
  - partitionTableType: gpt
    maxSize: 5G
    partitions:
    - id: esp
      type: esp
      size: 8M

    - id: boot
      size: 1G

    - label: root
      id: root
      size: 2G

    - label: root-hash
      id: verityhash
      size: 128M

    - id: var
      size: grow

  bootType: efi

  verity:
  - id: rootverity
    name: root
    dataDeviceId: root
    hashDeviceId: verityhash
    dataDeviceMountIdType: part-label
    hashDeviceMountIdType: part-label

  filesystems:
  - deviceId: esp
    type: fat32
    mountPoint:
      path: /boot/efi
      options: umask=0077

  - deviceId: boot
    type: ext4
    mountPoint:
      path: /boot

  - deviceId: rootverity
    type: ext4
    mountPoint:
      path: /
      options: defaults,ro

  - deviceId: var
    type: ext4
    mountPoint:
      path: /var

os:
  bootloader:
    resetType: hard-reset
  hostname: root-verity-image

  selinux:
    mode: enforcing

  kernelCommandLine:
    extraCommandLine:
    - log_buf_len=1M

  packages:
    remove:
      - grub2-efi-binary

    install:
      # replace grub2-efi-binary with grub2-efi-binary-noprefix
      - grub2-efi-binary-noprefix
      - curl
      - device-mapper
      - dracut-overlayfs
      - efibootmgr
      - iproute
      - iptables
      - lsof
      - lvm2
      - mdadm
      - netplan
      - openssh-server
      - systemd-udev
      - tpm2-tools
      - trident-service
      - veritysetup
      - vim

  services:
    enable:
    - trident
```

### Step 4: Invoke Image Customizer

Assuming RPMs, a base image `image.vhdx` and Image Customizer configuration `ic-config.yaml` found in `$HOME/staging`.

Invoke Image Customizer, paying special attention to [specify](https://microsoft.github.io/azure-linux-image-tools/imagecustomizer/api/cli.html#--output-image-formatformat) `--output-image-format=cosi`:

``` bash
pushd $HOME/staging
docker run \
    --rm \
    --privileged=true \
    -v /dev:/dev \
    -v ".:/staging:z" \
    mcr.microsoft.com/azurelinux/imagecustomizer:0.18.0 \
        --image-file "/staging/image.vhdx" \
        --config-file "/staging/ic-config.yaml" \
        --rpm-source "/staging/RPMS/x86_64" \
        --build-dir "/build" \
        --output-image-format "cosi" \
        --output-image-file "/staging/out/image.cosi"
popd
```
