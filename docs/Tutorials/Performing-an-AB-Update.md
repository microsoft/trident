
# Performing an A/B Update

## Introduction

Trident can be used to service an operating system, running either on bare metal or virtual machines.  To accomplish this, Trident uses an [A/B Update](../Reference/Glossary.md#ab-update) strategy.

This document describes how to build an update image, create an update Trident Host Configuration, and execute an update with `trident update`.

## Prerequisites

1. Ensure that [oras](https://oras.land/docs/installation/) is installed.
2. Ensure [Image Customizer container](https://microsoft.github.io/azure-linux-image-tools/imagecustomizer/quick-start/quick-start.html) is accessible.
3. Ensure SSH key pair exists (assumed in this tutorial to be `$HOME/.ssh/id_rsa.pub`)
4. A bare metal machine (via [Hello World](./Trident-Hello-World.md)) or virtual machine (via [Onboard a VM to Trident](./Onboard-a-VM-to-Trident.md)) has been provisioned.

## Instructions

### Step 1: Download the minimal base image

Pull [minimal-os](../Reference/Glossary.md#minimal-os) as a base image from MCR by running:

``` bash
mkdir -p $HOME/staging
pushd $HOME/staging
oras pull mcr.microsoft.com/azurelinux/3.0/image/minimal-os:latest
popd
```

### Step 2: Build Trident RPMs

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

### Step 3: Define an Update COSI Configuration

For an update COSI, we need to provide only an esp and the updated partitions. The `trident` partition does not have an [A/B volume pair](../Reference/Glossary.md#ab-volume-pair) and does not need to be serviced, so it is not included. The same would go for any data or other none-serviced partition.

Image Customizer reflects the update OS image, which will be laid out onto a single partition at a time: either A _or_ B. So, the [A/B volume pairs](../Reference/Glossary.md#ab-volume-pair) will not be reflected in the Image Customizer config.  This is why `root` is specified here unlike the `root-a` or `root-b` found in the Trident Host Configuration.

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

The final step for A/B Update is [trident commit](../Reference/Trident-CLI.md#commit). This will validate and ensure the machine's boot order is correct after an update. To enable `trident commit` in the update image, the `trident-service` package must be installed and the `trident` service needs to be enabled:

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

### Step 4: Create an Update COSI 

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

### Step 5: Create Trident Host Configuration for Update

To update our existing installation, we need a new Host Configuration file. In this case, we are only changing the OS based on a new COSI file that was created in step 4. In essence, the Host Configuration file used to deploy the initial operating system can be used as a basis, only changing the COSI file reference:

  ``` yaml
    image:
        url: /tmp/osimage-update.cosi
        sha384: ignored
  ```

Assuming a disk path of `/dev/sda` and a local COSI file, the Trident update Host Configuration can be created like this:

``` bash
DISK_DEVICE_PATH="/dev/sda"
cat << EOF > $HOME/staging/host-config-update.yaml
image:
  url: /tmp/osimage-update.cosi
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

### Step 6: Copy COSI and Host Configuration to the Servicing OS

While Trident can download COSI files from an OCI or http server, in this tutorial, we will just copy the COSI to a known location. This is based on knowing the IP address of the target machine, which can be supplied below as `TARGET_MACHINE_IP`:

``` bash
TARGET_MACHINE_IP="<IP ADDRESS>"
# Use SSH Copy to move the update Host Configuration to target machine
scp -i $HOME/.ssh/id_rsa $HOME/staging/host-config-update.yaml tutorial-user@$TARGET_MACHINE_IP:/tmp/host-config-update.yaml
# Use SSH Copy to move the update COSI to target machine
scp -i $HOME/.ssh/id_rsa $HOME/staging/osimage-update.cosi tutorial-user@$TARGET_MACHINE_IP:/tmp/osimage-update.cosi
```

### Step 7: Update OS on the Target Machine to B

To update the target machine, we will invoke [`trident update`](../Reference/Trident-CLI.md#update).

``` bash
TARGET_MACHINE_IP="<IP ADDRESS>"
# Use SSH to start an update
ssh -i $HOME/.ssh/id_rsa tutorial-user@$TARGET_MACHINE_IP trident update /tmp/host-config-update.yaml
```

## Conclusion

We have now seen how to build an update image, Trident Host Configuration, and how to invoke update.
