
# Building a Servicing ISO

## Introduction

The goal of this document is to produce an installation ISO that utilizes
Trident to install an Azure Linux operating system.

## Prerequisites

1. Ensure that [oras](https://oras.land/docs/installation/) is installed.
2. Ensure [Image Customizer
   container](https://microsoft.github.io/azure-linux-image-tools/imagecustomizer/quick-start/quick-start.html)
   is accessible.
3. Create an [Azure Linux image COSI
   file](./Building-AB-Update-Images-for-Install-and-Update.md), assuming the
   output COSI file is `$HOME/staging/osimage.cosi`.
4. Create a [Trident Host Configuration
   file](./Writing-a-Simple-Host-Configuration.md), assuming the file is
   `$HOME/staging/host-config.yaml`. For this document, the Host Configuration
   is assumed to reference the COSI file as being contained in the installer ISO
   at `/images/azure-linux.cosi`.

## Instructions

### Step 1: Download the minimal base image

Pull [minimal-os](../Reference/Glossary.md#minimal-os) as a base image from MCR
by running:

``` bash
mkdir -p $HOME/staging
pushd $HOME/staging
oras pull mcr.microsoft.com/azurelinux/3.0/image/minimal-os:latest --platform linux/amd64
popd
```

### Step 2: Build Trident RPMs

Build the Trident RPMs by running:

``` bash
make bin/trident-rpms.tar.gz
```

After running this make command, the RPMs will be built and packaged into
`bin/trident-rpms.tar.gz` and unpacked into `bin/RPMS/x86_64`:

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

### Step 3: Create an Image Customizer Configuration

Assuming locations for the Azure Linux image COSI file
(`$HOME/staging/osimage.cosi`) and the Trident Host Configuration file
(`$HOME/staging/host-config.yaml`), follow the [Image Customizer
documentation](https://microsoft.github.io/azure-linux-image-tools/imagecustomizer/how-to/live-iso.html)
to create an Image Customizer configuration file,
`$HOME/staging/ic-config.yaml`:

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
    - source: host-config.yaml
      destination: /etc/trident/config.yaml
scripts:
  postCustomization:
    - content: |
        # Use more intuitive path for the ISO mount
        ln -s /run/initramfs/live /mnt/trident_cdrom
iso:
  additionalFiles:
    - source: osimage.cosi
      destination: /images/azure-linux.cosi
```

### Step 4: Create Installation ISO

Assuming locations for the base image file (`$HOME/staging/image.vhdx`) and the
Image Customizer configuration file (`$HOME/staging/ic-config.yaml`), follow the
[Image Customizer
documentation](https://microsoft.github.io/azure-linux-image-tools/imagecustomizer/quick-start/quick-start.html)
and invoke Image Customizer:

``` bash
pushd $HOME/staging
docker run --rm \
    --privileged \
    -v ".:/files:z" \
    -v "/dev:/dev" \
    --platform linux/amd64 \
    mcr.microsoft.com/azurelinux/imagecustomizer:0.18.0 \
    --log-level debug \
    --build-dir /build \
    --image-file "/files/image.vhdx" \
    --rpm-source "/files/RPMS/x86_64" \
    --output-image-file "/files/installer.iso" \
    --config-file "/files/ic-config.yaml" \
    --output-image-format iso
popd
```
