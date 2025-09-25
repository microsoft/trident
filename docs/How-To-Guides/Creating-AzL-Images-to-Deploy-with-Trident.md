
# Creating Azure Linux Images to Deploy with Trident

## Goals

To deploy an operating system, Trident requires [COSI](../Reference/COSI.md) files. This document describes how to create a COSI file.

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

Build the Trident RPMs using `make bin/trident-rpms.tar.gz`.  After running this make command, the RPMs will be built and packaged into bin/trident-rpms.tar.gz and unpacked into bin/RPMS/x86_64:

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

Follow the Image Customizer [documentation](https://microsoft.github.io/azure-linux-image-tools/imagecustomizer/README.html) to configure `$HOME/staging/ic-config.yaml`:

``` yaml
storage:
  bootType: efi

  disks:
    - partitionTableType: gpt
      maxSize: 4G
      partitions:
        - id: esp
          type: esp
          size: 8M

        - id: rootfs
          size: grow

  filesystems:
    - deviceId: esp
      type: fat32
      mountPoint:
        path: /boot/efi
        options: umask=0077

    - deviceId: rootfs
      type: ext4
      mountPoint:
        path: /

os:
  bootloader:
    resetType: hard-reset
  hostname: testimage

  kernelCommandLine:
    extraCommandLine:
      - rd.info
      - log_buf_len=1M

  packages:
    remove:
      - grub2-efi-binary

    install:
      # replace grub2-efi-binary with grub2-efi-binary-noprefix
      - grub2-efi-binary-noprefix
      - curl
      - dnf
      - efibootmgr
      - iproute
      - iptables
      - lsof
      - mdadm
      - netplan
      - openssh-server
      - tpm2-tools
      - trident-service
      - vim
      - audit

  services:
    enable:
      - sshd
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
