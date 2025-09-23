
# Create Install Media

<!--
DELETE ME AFTER COMPLETING THE DOCUMENT!
---
Task: https://dev.azure.com/mariner-org/polar/_workitems/edit/13136
Title: Create Install Media
Type: How-To Guide
Objective:

Guide the user through the process of creating install media for AzL 3.0. Refer
to guide on creating runtime images.
-->

## Goals

The goal of this document is to produce an installation ISO that utilizes Trident to install an Azure Linux operating system.

## Prerequisites

1. Ensure [Image Customizer container is accessible](https://microsoft.github.io/azure-linux-image-tools/imagecustomizer/quick-start/quick-start.html).
2. [Download a base image](https://microsoft.github.io/azure-linux-image-tools/imagecustomizer/how-to/download-marketplace-image.html) for the installer ISO
3. Create an [Azure Linux image COSI file](../Tutorials/Building-a-Deployable-Image.md).
4. Create a Trident host configuration file. For this document, the host configuration is assumed to reference the COSI file as being contained in the installer ISO at `/images/azure-linux.cosi`.

## Instructions

### Step 1: Create an Image Customizer Configuration

Assuming locations for the Azure Linux image COSI file (`./files/osimage.cosi`) and the Trident host configuration file (`./files/host-config.yaml`), create a configuration file for the Image Customizer, following the [Image Customizer documentation](https://microsoft.github.io/azure-linux-image-tools/imagecustomizer/how-to/live-iso.html).

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
  hostname: installer-iso-mos
  bootloader:
    resetType: hard-reset
  selinux:
    mode: enforcing
  kernelCommandLine:
    extraCommandLine:
      - rd.info
      - console=ttyS0
      - console=tty0
  packages:
    install:
      - netplan
      - trident-install-service
      - trident-provisioning
      - vim
      - curl
      - device-mapper
      - squashfs-tools
      - tar
      - selinux-policy
  services:
    enable:
      - trident-install.service
      - trident-network.service
  additionalFiles:
    # DO WE NEED TO DOCUMENT THESE?
    # - source: files/getty@.service
    #   destination: /usr/lib/systemd/system/getty@.service
    # - source: files/serial-getty@.service
    #   destination: /usr/lib/systemd/system/serial-getty@.service
    # - source: files/root.profile
    #   destination: /root/.profile
    - source: files/host-config.yaml
      destination: /etc/trident/config.yaml
scripts:
  postCustomization:
    - content: |
        # Use more intuitive path for the ISO mount
        ln -s /run/initramfs/live /mnt/trident_cdrom
iso:
  additionalFiles:
    - source: files/osimage.cosi
      destination: /images/azure-linux.cosi

```

### Step 2: Invoke Image Customizer to Create Installation ISO

Assuming locations for the base image file (`./files/baseimage.vhdx`) and the Image Customizer configuration file (`./files/ic-config.yaml`), invoke Image Customizer, following the [Image Customizer documentation](https://microsoft.github.io/azure-linux-image-tools/imagecustomizer/quick-start/quick-start.html).

``` bash
docker run --rm \
    --privileged \
    -v "./files:/files:z" \
    -v "/dev:/dev" \
    --platform linux/amd64 \
    mcr.microsoft.com/azurelinux/imagecustomizer:0.18.0 \
    --log-level debug \
    --build-dir /build \
    --image-file /files/baseimage.vhdx \
    --output-image-file /files/installer.iso \
    --config-file /files/ic-config.yaml \
    --output-image-format iso

```
