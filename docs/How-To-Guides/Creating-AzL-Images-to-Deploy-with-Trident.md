
# Creating AzL Images to Deploy with Trident

## Goals

To deploy an operating system, Trident requires [COSI](../Reference/COSI.md) files. This document describes how to create a COSI file.

## Prerequisites

1. [Install Image Customizer](https://microsoft.github.io/azure-linux-image-tools/imagecustomizer/quick-start/quick-start.html).

## Instructions

### Create OS Image

Follow the Image Customizer [documentation](https://microsoft.github.io/azure-linux-image-tools/imagecustomizer/quick-start/quick-start-binary.html) to configure and create an OS image, paying special attention to [specify](https://microsoft.github.io/azure-linux-image-tools/imagecustomizer/api/cli.html#--output-image-formatformat) `--output-image-format=cosi`.

For example, an Image Customizer configuration creating a simple image might look like:

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

could be built with Image Customizer using a command like this (assuming a base image `image.vhdx` and Image Customizer configuration `image-config.yaml` found in `$HOME/staging`):

``` bash
 docker run \
   --rm \
   --privileged=true \
   -v /dev:/dev \
   -v "$HOME/staging:/mnt/staging:z" \
   mcr.microsoft.com/azurelinux/imagecustomizer:0.18.0 \
     --image-file "/mnt/staging/image.vhdx" \
     --config-file "/mnt/staging/image-config.yaml" \
     --build-dir "/build" \
     --output-image-format "cosi" \
     --output-image-file "/mnt/staging/out/image.cosi"
```