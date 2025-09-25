
# Building a Deployable Image

<!--
DELETE ME AFTER COMPLETING THE DOCUMENT!
---
Task: https://dev.azure.com/mariner-org/polar/_workitems/edit/13123
Title: Building a Deployable Image
Type: Tutorial
Objective:

Very hand-holdy tutorial on how to build an image with Prism.

The image should have AB update enabled so we can use it in future tutorials!
-->

## Introduction

To deploy an operating system, Trident requires [COSI](../Reference/COSI.md) files for both install and servicing.

This document describes how to create a COSI file.

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

Follow the Image Customizer [documentation](https://microsoft.github.io/azure-linux-image-tools/imagecustomizer/README.html) to create a configuration file.

To support A/B Update, updatable filesystems require both an A and a B partition.  This allows Trident to stage a new update while the current operating system runs uninterrupted.  For example, to configure A and B partitions for root (`/`), we follow the `-a` and `-b` suffix convention like this when defining disks:

``` yaml
  disks:
    - partitionTableType: gpt
      maxSize: 10G
      partitions:
        - id: root-a
          size: 4G
        - id: root-b
          size: 4G
```

This will set up the required partitions, but this is only half of the required configuration.  We need to tell Image Customizer where to put the root filesystem.  To do so, configure the filesystem to use `root-a` like this:

``` yaml
  filesystems:
    - deviceId: root-a
      type: ext4
      mountPoint:
        path: /
```

A/B partition pairs must exist for any partitions that are serviced as part of an update.

While these A/B partition pairs are vital to A/B Update, there are a some filesystems that cannot be hosted on A/B partitions. These filesystems retain state between the A and B operating systems.

`/boot/efi` contains state that dictates boot and can be defined like this:

``` yaml
  disks:
    - partitionTableType: gpt
      maxSize: 10G
      partitions:
        - id: esp
          type: esp
          size: 8M

  filesystems:
    - deviceId: esp
      type: fat32
      mountPoint:
        path: /boot/efi
        options: umask=0077
```

`/var/lib/trident` is the default location Trident uses for its datastore and can be defined like this:

``` yaml
  disks:
    - partitionTableType: gpt
      maxSize: 10G
      partitions:
        - id: trident
          size: 100M
          type: linux-generic

  filesystems:
    - deviceId: trident
      type: ext4
      mountPoint:
        path: /var/lib/trident

```

In addition to partition and filesystem definition, Trident must be added to the image.  As the final step in install and update, [trident commit](../Reference/Trident-CLI.md#commit) must be invoked to validate an update and ensure the machine's boot order is correct.  To enable commit, Trident needs the `trident-service` package to be installed and the `trident` service to be enabled:

``` yaml
  packages:
    install:
      - trident-service

  services:
    enable:
      - trident
```

To put all of that together, create the Image Customization configuration file like this:

``` bash
cat << EOF > $HOME/staging/ic-config.yaml
storage:
  bootType: efi

  disks:
    - partitionTableType: gpt
      maxSize: 10G
      partitions:
        - id: esp
          type: esp
          size: 8M
        - id: root-a
          size: 4G
        - id: root-b
          size: 4G
        - id: trident
          size: 100M
          type: linux-generic
          label: trident

  filesystems:
    - deviceId: esp
      type: fat32
      mountPoint:
        path: /boot/efi
        options: umask=0077
    - deviceId: root-a
      type: ext4
      mountPoint:
        path: /
    - deviceId: trident
      type: ext4
      mountPoint:
        path: /var/lib/trident

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
EOF
```

### Step 4: Invoke Image Customizer

From previous steps, the Trident RPMs, a base image (`image.vhdx`) and Image Customizer configuration file (`ic-config.yaml`) are all found in `$HOME/staging`.

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

## Conclusion

This COSI file (`out/image.cosi`) can now be used by Trident during install or update.