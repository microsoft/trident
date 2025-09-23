
# Building a Provisioning ISO

## Introduction

The goal of this document is to produce an installation ISO that utilizes Trident to install an Azure Linux operating system.

## Prerequisites

1. Ensure that [oras](https://oras.land/docs/installation/) is installed.
2. Ensure [Image Customizer container is accessible](https://microsoft.github.io/azure-linux-image-tools/imagecustomizer/quick-start/quick-start.html).
3. Create an [Azure Linux image COSI file](./Building-a-Deployable-Image.md), assuming the output COSI file is `file/osimage.cosi`.
4. Create a [Trident host configuration file](./Writing-a-Simple-Host-Configuration.md), assuming the file is `file/host-config.yaml`. For this document, the host configuration is assumed to reference the COSI file as being contained in the installer ISO at `/images/azure-linux.cosi`

## Instructions

### Step 1: Get Trident RPMs

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

### Step 2: Download the minimal base image

Pull the minimal base image from mcr by running `oras pull mcr.microsoft.com/azurelinux/3.0/image/minimal-os:latest`

The minimal base image will be saved as `image.vhdx` in the current directory.

``` bash
$ ls -lh image*
-rw-rw-r-- 1 bfjelds bfjelds 600M Sep 23 18:02 image.vhdx
-rw-rw-r-- 1 bfjelds bfjelds  97K Sep 23 18:02 image.vhdx.spdx.json
-rw-rw-r-- 1 bfjelds bfjelds  11K Sep 23 18:02 image.vhdx.spdx.json.sig
```

### Step 3: Create an Image Customizer Configuration

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

### Step 4: Invoke Image Customizer to Create Installation ISO

Assuming locations for the base image file (`./files/image.vhdx`) and the Image Customizer configuration file (`./files/ic-config.yaml`), invoke Image Customizer, following the [Image Customizer documentation](https://microsoft.github.io/azure-linux-image-tools/imagecustomizer/quick-start/quick-start.html).

``` bash
docker run --rm \
    --privileged \
    -v "./files:/files:z" \
    -v "/dev:/dev" \
    --platform linux/amd64 \
    mcr.microsoft.com/azurelinux/imagecustomizer:0.18.0 \
    --log-level debug \
    --build-dir /build \
    --image-file /files/image.vhdx \
    --output-image-file /files/installer.iso \
    --config-file /files/ic-config.yaml \
    --output-image-format iso

```
