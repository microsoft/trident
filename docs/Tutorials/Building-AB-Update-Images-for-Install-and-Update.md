
# Building A/B Update Images for Install and Update

## Introduction

To deploy an operating system, Trident requires [COSI](../Reference/COSI.md) files for both install and update.

This document describes how to create COSI files that support A/B Update for both install and update.

## Prerequisites

1. Ensure that [oras](https://oras.land/docs/installation/) is installed.
2. Ensure [Image Customizer container](https://microsoft.github.io/azure-linux-image-tools/imagecustomizer/quick-start/quick-start.html) is accessible.
3. Ensure SSH Key Pair Exists (assumed in this tutorial to be `$HOME/.ssh/id_rsa.pub`)

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

### Step 3: Create Image Customizer Configuration for Install

Follow the Image Customizer [documentation](https://microsoft.github.io/azure-linux-image-tools/imagecustomizer/README.html) to create a configuration file.

To support [A/B update](../Reference/Glossary.md#ab-update), updatable filesystems require both an A and a B volume.  This allows Trident to stage a new update while the current operating system runs uninterrupted.  For example, to configure A and B volumes for root (`/`), we follow the `-a` and `-b` suffix convention like this when defining disks:

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

[A/B volume pairs](../Reference/Glossary.md#ab-volume-pair) must exist for any volumes that are going to be serviced as part of an update.

While these A/B volume pairs are vital to A/B Update, there are a some filesystems that cannot be hosted on A/B volumes. These filesystems retain state between the A and B operating systems.

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

In addition to partition and filesystem definition, Trident must be added to the image.  As the final step in install, [trident commit](../Reference/Trident-CLI.md#commit) must be invoked to validate and ensure the machine's boot order is correct.  To enable commit, Trident needs the `trident-service` package to be installed and the `trident` service to be enabled:

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
cat << EOF > $HOME/staging/ic-config-install.yaml
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

### Step 4: Invoke Image Customizer to Create an Install COSI File

From previous steps, the Trident RPMs, a base image (`image.vhdx`) and Image Customizer configuration file (`ic-config-install.yaml`) are all found in `$HOME/staging`.

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
        --config-file "/staging/ic-config-install.yaml" \
        --rpm-source "/staging/RPMS/x86_64" \
        --build-dir "/build" \
        --output-image-format "cosi" \
        --output-image-file "/staging/osimage.cosi"
popd
```

### Step 5: Create Trident Host Configuration for Install

Create a Trident host configuration file that aligns to the Image Customizer COSI that was created in step 4.  The esp, root A/B volume pair, and trident partitions/filesystems should reflect what was specified in the Image Customizer configuration.

Trident does require a little more information about the A/B volume pair.  For that, we create an `abUpdate` section where the underlying `root-a` and `root-b` are linked to a logical `root` volume:

``` yaml
  abUpdate:
    volumePairs:
      - id: root
        volumeAId: root-a
        volumeBId: root-b
```

The remainder of the Trident host configuration file describes things like where to find the COSI file (in this case, the url will be a local path), what the disk device path is (in this case, /dev/sda), some user data (including the public key `$HOME/.ssh/id_rsa.pub`), some network setup, and an selinux configuration:

``` bash
DISK_DEVICE_PATH="/dev/sda"
cat << EOF > $HOME/staging/host-config.yaml
image:
  url: /images/azure-linux.cosi
  sha384: ignored
storage:
  disks:
    - id: os
      device: $DISK_DEVICE_PATH
      partitionTableType: gpt
      partitions:
        - id: esp
          type: esp
          size: 1G
        - id: root-a
          type: root
          size: 8G
        - id: root-b
          type: root
          size: 8G
        - id: trident
          type: linux-generic
          size: 1G
  abUpdate:
    volumePairs:
      - id: root
        volumeAId: root-a
        volumeBId: root-b
  filesystems:
    - deviceId: root
      mountPoint: /
    - deviceId: esp
      mountPoint:
        path: /boot/efi
        options: umask=0077
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
  users:
    - name: tutorial-user
      sshPublicKeys:
        - "$(cat $HOME/.ssh/id_rsa.pub)"
      sshMode: key-only
EOF
```

### Step 6: Create Servicing ISO and Install the A OS

To install the COSI we created in step 4, we need to create a servicing ISO. Follow the [Building a Servicing ISO tutorial](./Building-a-Servicing-ISO.md), using the COSI created in step 4 and the host configuration created in step 5 as the tutorial's prerequisites.

You can create a bootable USB stick from the servicing ISO by using a tool like Rufus (or any similar tool).  This USB stick can be used to install the `A` operating system.

Alternatively, to simulate an installation, you can create a virtual machine with an empty disk and mount the ISO directly as a CD.

### Step 7: Create Image Customizer Configuration for Update

The process for creating an update COSI file is similar to what we did for Install.

An important difference is that for update, we only need to provide an esp and the updated partitions (note that `root` is specified, there is no `root-a` or `root-b`, there is no `trident`):

``` yaml
  disks:
    - maxSize: 5G
      partitions:
        - id: esp
          size: 8M
          type: esp
        - id: root
          size: 4G
```

Similarly, filesystems should only contain entries for esp and the updated filesystems:

``` yaml
  filesystems:
    - deviceId: esp
      mountPoint:
        options: umask=0077
        path: /boot/efi
      type: fat32
    - deviceId: root
      mountPoint: /
      type: ext4
```

As for the install COSI, Trident must be added.  The final step for update is [trident commit](../Reference/Trident-CLI.md#commit), which must be invoked to validate and ensure the machine's boot order is correct.  To enable commit, Trident needs the `trident-service` package to be installed and the `trident` service to be enabled:

``` yaml
  packages:
    install:
      - trident-service

  services:
    enable:
      - trident
```

To put all of that together, create the update Image Customization configuration file like this:

``` bash
cat << EOF > $HOME/staging/ic-config-update.yaml
storage:
  bootType: efi
  disks:
    - maxSize: 5G
      partitionTableType: gpt
      partitions:
        - id: esp
          size: 8M
          type: esp
        - id: root
          size: 4G

  filesystems:
    - deviceId: esp
      mountPoint:
        options: umask=0077
        path: /boot/efi
      type: fat32
    - deviceId: root
      mountPoint: /
      type: ext4

os:
  bootloader:
    resetType: hard-reset
  hostname: updated-testimage

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
      - jq
      - lsof
      - netplan
      - openssh-server
      - trident-service
      - vim

  services:
    enable:
      - trident
EOF
```

### Step 8: Invoke Image Customizer to Create an Update COSI File

From previous steps, the Trident RPMs, a base image (`image.vhdx`) and Image Customizer configuration file (`ic-config-update.yaml`) are all found in `$HOME/staging`.

Invoke Image Customizer, again paying special attention to [specify](https://microsoft.github.io/azure-linux-image-tools/imagecustomizer/api/cli.html#--output-image-formatformat) `--output-image-format=cosi`:

``` bash
pushd $HOME/staging
docker run \
    --rm \
    --privileged=true \
    -v /dev:/dev \
    -v ".:/staging:z" \
    mcr.microsoft.com/azurelinux/imagecustomizer:0.18.0 \
        --image-file "/staging/image.vhdx" \
        --config-file "/staging/ic-config-update.yaml" \
        --rpm-source "/staging/RPMS/x86_64" \
        --build-dir "/build" \
        --output-image-format "cosi" \
        --output-image-file "/staging/osimage-update.cosi"
popd
```

### Step 9: Create Trident Host Configuration for Update

To update our existing installation, we need a new host configuration file. In this case, we are only changing the OS based on a new COSI file. So we can simply copy the install and update the image url to point at our new COSI file.

``` bash
# Create an update version of the host configuration
cp $HOME/staging/host-config.yaml $HOME/staging/host-config-update.yaml
# Modify the image url to point at /tmp
sed -i 's|url: /images/azure-linux.cosi|url: /tmp/osimage-update.cosi|' $HOME/staging/host-config-update.yaml
```

### Step 10: Copy COSI and Host Configuration to OS

While Trident can download COSI files from an OCI or http server, in this tutorial, we will just copy the COSI to a known location. This is based on knowing the IP address of the target machine, which can be supplied below as `TARGET_MACHINE_IP`:

``` bash
TARGET_MACHINE_IP="<IP ADDRESS>"
# Use SSH Copy to move the update host configuration to target machine
scp -i $HOME/.ssh/id_rsa $HOME/staging/host-config-update.yaml tutorial-user@$TARGET_MACHINE_IP:/tmp/host-config-update.yaml
# Use SSH Copy to move the update COSI to target machine
scp -i $HOME/.ssh/id_rsa $HOME/staging/osimage-update.cosi tutorial-user@$TARGET_MACHINE_IP:/tmp/osimage-update.cosi
```

### Step 11: Update Target Machine to B OS

To update the target machine, we will invoke [`trident update`](../Reference/Trident-CLI.md#update).

``` bash
TARGET_MACHINE_IP="<IP ADDRESS>"
# Use SSH to start an update
ssh -i $HOME/.ssh/id_rsa tutorial-user@$TARGET_MACHINE_IP trident update /tmp/host-config-update.yaml
```

## Conclusion

We have created A/B Update capable COSI files for both install and update.  We have seen how to create Image Customizer configurations, how to invoke Image Customizer, and how to create matching Trident host configuration files.
