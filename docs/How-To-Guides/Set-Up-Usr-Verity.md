
# Set Up Usr Verity

## Goals

Configuring [usr-verity](../Explanation/Usr-Verity.md) offers good protection against modification of the root (`/usr`) partition.

The goal of this document is to enable you to create a [COSI](../Reference/COSI.md) file that sets up usr-verity.

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

To create a usr-verity volume, there are a few Image Customizer configuration sections that are important.

In addition to the typical `usr` partition definition, a `usr-hash` partition is needed like this:

``` yaml
storage:
  disks:
    - partitionTableType: gpt
      partitions:
        - id: usr-hash
          label: usr-hash
          size: 128M
```

The [Image Customizer verity section](https://microsoft.github.io/azure-linux-image-tools/imagecustomizer/api/configuration/verity.html) is required as well:

``` yaml
verity:
  - id: usr
    name: usr
    dataDeviceId: usr-data
    hashDeviceId: usr-hash
    dataDeviceMountIdType: uuid
    hashDeviceMountIdType: uuid
```

Verity filesystems should be created as read-only:

``` yaml
- deviceId: usr
  type: ext4
  mountPoint:
    path: /usr
    options: defaults,ro
```

And finally, usr-verity requires some changes to support UKI rather than grub:

``` yaml
os:
  kernelCommandLine:
    extraCommandLine:
      - rd.hostonly=0

  uki:
    kernels: auto

previewFeatures:
  - uki
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
          size: 500M
          label: esp

        - id: boot
          size: 150M

        - id: root
          size: 2G

        - id: usr-data
          label: usr
          size: 1G

        - id: usr-hash
          label: usr-hash
          size: 128M

  bootType: efi

  verity:
    - id: usr
      name: usr
      dataDeviceId: usr-data
      hashDeviceId: usr-hash
      dataDeviceMountIdType: uuid
      hashDeviceMountIdType: uuid

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

    - deviceId: root
      type: ext4
      mountPoint:
        path: /

    - deviceId: usr
      type: ext4
      mountPoint:
        path: /usr
        options: defaults,ro

os:
  bootloader:
    resetType: hard-reset
  hostname: root-verity-image

  selinux:
    mode: enforcing

  kernelCommandLine:
    extraCommandLine:
      - log_buf_len=1M
      - rd.hostonly=0

  packages:
    remove:
      - grub2-efi-binary

    install:
      - binutils
      - curl
      - device-mapper
      - efibootmgr
      - iproute
      - iptables
      - lvm2
      - mdadm
      - netplan
      - openssh-server
      - systemd-udev
      - tpm2-tools
      - trident-service
      - trident-static-pcrlock-files
      - veritysetup
      - vim
      - systemd-ukify
      - systemd-boot
      - audit
      - selinux-policy-devel

  services:
    enable:
      - trident
  uki:
    kernels: auto

previewFeatures:
  - uki
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

### Step 5: Trident Host Configuration

Create a Trident host configuration file that aligns to the Image Customizer COSI that was created in step 4. The esp, boot, root, usr, and usr-hash partitions/filesystems should reflect what was specified in the Image Customizer configuration.

Some things to note that are defined in the host configuration below:

* [A/B volume pairs](../Reference/Glossary.md#ab-volume-pair) for `usr-data` and `usr-hash`
* [abUpdate section](../Reference/Host-Configuration/API-Reference/AbUpdate.md) for `usr-data` and `usr-hash`
* [verity section](../Reference/Host-Configuration/API-Reference/VerityDevice.md) to connect `usr` data and hash

The remainder of the Trident host configuration file describes things like where to find the COSI file (can be a local path, an HTTP url, or an OCI url) and what the disk device path is (in this case, /dev/sda):

``` yaml
image:
  url: image.cosi
  sha384: ignored
storage:
  disks:
    - id: os
      device: /dev/sda
      partitionTableType: gpt
      partitions:
        - id: esp
          type: esp
          size: 1G
        - id: boot
          type: xbootldr
          size: 200M
        - id: root
          type: root
          size: 5G
        - id: usr-data-a
          type: usr
          size: 5G
        - id: usr-data-b
          type: usr
          size: 5G
        - id: usr-hash-a
          type: usr-verity
          size: 1G
        - id: usr-hash-b
          type: usr-verity
          size: 1G
        - id: trident
          type: linux-generic
          size: 1G

  abUpdate:
    volumePairs:
      - id: usr-data
        volumeAId: usr-data-a
        volumeBId: usr-data-b
      - id: usr-hash
        volumeAId: usr-hash-a
        volumeBId: usr-hash-b

  verity:
    - id: usr
      name: usr
      dataDeviceId: usr-data
      hashDeviceId: usr-hash

  filesystems:
    - deviceId: esp
      mountPoint:
        path: /boot/efi
        options: umask=0077
    - deviceId: boot
      mountPoint: /boot
    - deviceId: root
      mountPoint: /
    - deviceId: usr
      mountPoint:
        path: /usr
        options: ro
    - deviceId: trident
      source: new
      mountPoint: /var/lib/trident

os:
  selinux:
    mode: enforcing
  netplan:
    version: 2
    ethernets:
      vmeths:
        match:
          name: enp*
        dhcp4: true
```
