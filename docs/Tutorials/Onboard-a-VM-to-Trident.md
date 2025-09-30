
# Onboard a VM to Trident

## Introduction

For Trident to be able to update an existing installation of Azure Linux, it needs to track information such as partition and disk layout. This information is discovered by [offline-initialize](../Reference/Trident-CLI.md#offline-initialize).

This document will outline the steps required to enable this during image creation with Image Customizer.

## Prerequisites

1. Ensure [Image Customizer container](https://microsoft.github.io/azure-linux-image-tools/imagecustomizer/quick-start/quick-start.html) is accessible.

## Instructions

### Step 1: Download the minimal base image

Pull [minimal-os](../Reference/Glossary.md#minimal-os) as a base image from MCR by running:

``` bash
mkdir -p $HOME/staging
pushd $HOME/staging
oras pull mcr.microsoft.com/azurelinux/3.0/image/minimal-os:latest --platform linux/amd64
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

### Step 3: Create Image Customizer configuration including offline-initialize

Add the `trident-service` package to the Image Customizer configuration. This will add the Trident services needed for update and the `trident` package used for `offline-initialize`.

To invoke `trident offline-initialize` during image creation, add it in the `postCustomization` scripts.

These steps are shown below in a simple Image Customizer configuration (assumed as contents of `$HOME/staging/ic-config.yaml`):

``` yaml
storage:
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
          size: 1G
        - id: srv
          size: grow

  bootType: efi

  filesystems:
    - deviceId: esp
      type: fat32
      mountPoint:
        path: /boot/efi
        options: umask=0077
    - deviceId: root-a
      type: ext4
      mountPoint: /
    - deviceId: trident
      type: ext4
      mountPoint: /var/lib/trident
    - deviceId: srv
      type: ext4
      mountPoint: /srv

os:
  bootloader:
    resetType: hard-reset
  hostname: update-ready-os

  selinux:
    mode: enforcing

  kernelCommandLine:
    extraCommandLine:
      - log_buf_len=1M

  packages:
    install:
      - dnf
      - efibootmgr
      - grub2-efi-binary
      - iproute
      - iptables
      - jq
      - openssh-server
      - trident-service
      - vim

scripts:
  postCustomization:
    - content: |
        # Add the necessary directories for the audit logs so that auditd can start
        mkdir -p /var/log/audit
    - content: |
        trident offline-initialize
```

### Step 4: Invoke Image Customizer

Assuming the RPMs, a base image (`image.vhdx`) and the Image Customizer configuration file (`ic-config.yaml`) are found in `$HOME/staging`, invoke Image Customizer to create a qcow2 file:

``` bash
pushd $HOME/staging
docker run --rm \
    --privileged \
    -v ".:/staging:z" \
    -v "/dev:/dev" \
    mcr.microsoft.com/azurelinux/imagecustomizer:0.18.0 \
        --rpm-source /staging/RPMS/x86_64 \
        --build-dir /build \
        --image-file /staging/image.vhdx \
        --output-image-file /staging/image.qcow2 \
        --output-image-format qcow2 \
        --config-file /staging/ic-config.yaml
popd
```

## Conclusion

Using `image.qcow2` as the operating system for a virtual machine will create a machine that is ready for `trident update` to work!
