
# Set Up Root Verity

## Goals

Configuring [root-verity](../Explanation/Root-Verity.md) offers good protection against modification of the root (`/`) partition.

:::info

An alternative (both cannot be configured) is to instead configure [usr-verity](../Explanation/Usr-Verity.md) to protect against modification of the usr (`/usr`) partition.

:::

The goal of this document is to create a [Trident host configuration](../Reference/Host-Configuration/API-Reference/HostConfiguration.md) file and a [COSI](../Reference/Composable-OS-Image.md) file that can be used to install and service an image with a root-verity partition.

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

To create a root-verity volume, there are a few Image Customizer configuration sections that are important.

In addition to the typical `root` partition definition, a `root-hash` partition is needed like this:

``` yaml
storage:
  disks:
  - partitionTableType: gpt
    partitions:
    - label: root-hash
      id: root-hash
      size: 128M
```

The [Image Customizer verity section](https://microsoft.github.io/azure-linux-image-tools/imagecustomizer/api/configuration/verity.html) is required as well:

``` yaml
verity:
  - id: root
    name: root
    dataDeviceId: root-data
    hashDeviceId: root-hash
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

    - label: root-data
      id: root-data
      size: 2G

    - label: root-hash
      id: root-hash
      size: 128M

    - id: var
      size: grow

  bootType: efi

  verity:
  - id: root
    name: root
    dataDeviceId: root-data
    hashDeviceId: root-hash
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

  - deviceId: root
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

### Step 5: Trident Host Configuration

Create a Trident host configuration file that aligns to the Image Customizer COSI that was created in step 4. The esp, root, root-hash, and var partitions/filesystems should reflect what was specified in the Image Customizer configuration.

Some things to note that are defined in the host configuration below:

* [A/B volume pairs](../Reference/Glossary.md#ab-volume-pair) for `root-data` and `root-hash`
* [abUpdate section](../Reference/Host-Configuration/API-Reference/AbUpdate.md) for `root-data` and `root-hash`
* [verity section](../Reference/Host-Configuration/API-Reference/VerityDevice.md) to connect `root` data and hash

The remainder of the Trident host configuration file describes things like where to find the COSI file (can be a local path, an HTTP url, or an OCI url) and what the disk device path is (in this case, /dev/sda):

```yaml
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
        - id: root-data-a
          type: root
          size: 4G
        - id: root-data-b
          type: root
          size: 4G
        - id: root-hash-a
          type: root-verity
          size: 1G
        - id: root-hash-b
          type: root-verity
          size: 1G
        - id: var
          type: linux-generic
          size: 1G

  abUpdate:
    volumePairs:
      - id: root-data
        volumeAId: root-data-a
        volumeBId: root-data-b
      - id: root-hash
        volumeAId: root-hash-a
        volumeBId: root-hash-b

  verity:
    - id: root
      name: root
      dataDeviceId: root-data
      hashDeviceId: root-hash

  filesystems:
    - deviceId: esp
      mountPoint:
        path: /boot/efi
        options: umask=0077
    - deviceId: boot
      mountPoint: /boot
    - deviceId: var
      mountPoint: /var
    - deviceId: root
      mountPoint:
        path: /
        options: defaults,ro

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

## Troubleshooting

With root-verity, configurations can be difficult as the configuration files are often on the root partition.  In the future, this section will be expanded to include learnings and hints for how to navigate these challenges.
