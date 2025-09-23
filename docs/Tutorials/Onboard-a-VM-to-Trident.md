
# Onboard a VM to Trident

## Introduction

For Trident to be able to update an existing installation of Azure Linux, it needs to track information such as partition and disk layout.  This information can be discovered for Trident using `trident offline-initialize`.

This document will outline the steps required to enable this during image creation with Image Customizer.

## Prerequisites

1. Ensure [Image Customizer container is accessible](https://microsoft.github.io/azure-linux-image-tools/imagecustomizer/quick-start/quick-start.html).

## Instructions

### Step 1: Get Trident RPMs

Build the Trident RPMs using `make bin/trident-rpms.tar.gz`.  After running this make command, the RPMs will be built and packaged into bin/tridnet-rpms.tar.gz and unpacked into bin/RPMS/x86_64:

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

### Step 3: Create Image Customizer configuration including offline-initialize

ic-config.yaml:
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
      # Note: grub2-efi-binary-noprefix package is going to be the default grub package for azl3
      - grub2-efi-binary
      - iproute
      - iptables
      - jq
      - openssh-server
      - trident-service
      - vim

  additionalFiles:
    # TODO: DO WE NEED THIS??
    # - source: files/sshd-keygen.service
    #   destination: /usr/lib/systemd/system/sshd-keygen.service
    # - source: files/99-dhcp-eth0.network
    #   destination: /etc/systemd/network/99-dhcp-eth0.network
    # - source: files/sudoers-wheel
    #   destination: /etc/sudoers.d/wheel

scripts:
  postCustomization:
    - path: |
        # Add the necessary directories for the audit logs so that auditd can start
        mkdir -p /var/log/audit
    - path: |
        trident offline-initialize
```

``` bash
docker run --rm \
    --privileged \
    -v ".:/repo:z" \
    -v "/dev:/dev" \
    mcr.microsoft.com/azurelinux/imagecustomizer:0.18.0 \
        --rpm-source /repo/bin/RPMS/x86_64 \
        --build-dir /build \
        --image-file /repo/image.vhdx \
        --output-image-file /repo/image.qcow2 \
        --output-image-format qcow2 \
        --config-file /repo/ic-config.yaml
```

## Conclusion

Using `image.qcow2` as the operating system for a virtual machine will create a machine that is ready for `trident update` to work!