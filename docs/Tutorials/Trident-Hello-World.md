# Trident Hello World

In this tutorial, we will install Azure Linux on a machine using Trident. You
will see firsthand how Trident transforms a blank machine into a fully
configured Azure Linux system in just a few minutes!

## Introduction

Trident is a declarative OS lifecycle agent. It is designed to perform clean
installs on bare-metal hosts, but for learning/demonstration purposes, a virtual
machine can also be used. We'll boot from the `Servicing ISO`, and use Trident
to install and configure Azure Linux.

## Prerequisites

Before we start, you'll need to:

1. Ensure that [oras](https://oras.land/docs/installation/) is installed.
2. Ensure [Image Customizer
   container](https://microsoft.github.io/azure-linux-image-tools/imagecustomizer/quick-start/quick-start.html)
   is accessible.
3. A test target system
   - Either a physical machine for bare-metal installation, OR
   - A virtual machine for testing (see [Appendix: Virtual Machine
     Setup](#appendix-virtual-machine-setup))
4. System resources
   - At least 16GB of available disk space on the target system
   - 4GB of available RAM
   - Administrative access

## Instructions

### Step 1: Create the COSI file and Host Configuration

Follow the [Building A/B Update Images for Install and
Update](./Building-AB-Update-Images-for-Install-and-Update.md) tutorial through
[Step 5: Create Trident Host Configuration for
Install](./Building-AB-Update-Images-for-Install-and-Update.md#step-5-create-trident-host-configuration-for-install).
**Stop after completing Step 5** (do not create the Servicing ISO of Step 6, we
will create our own modified Servicing ISO in the next step). After following
those steps you will have:
- The [minimal-os](../Reference/Glossary.md#minimal-os)
  (`$HOME/staging/image.vhdx`)
- The Trident RPMs (`bin/RPMS/x86_64`)
- The COSI file (`$HOME/staging/osimage.cosi`)
- The Host Configuration file (`$HOME/staging/host-config.yaml`)

### Step 2: Build a Servicing ISO

#### 2.1 Create an Image Customizer Configuration

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

#### 2.2 Invoke Image Customizer to Create Installation ISO

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

### Step 3: Boot from the Servicing ISO

Use the tool of your choice to create bootable media from the Servicing ISO.

Insert the bootable media (USB, CD, etc.) into your target system and power it
on. Make sure to configure it to boot from the media first or select the media
during the subsequent boot using the appropriate key (often F12).

The system will boot into the Azure Linux installer environment.

### Step 4: Access the installer environment

After a few moments, the screen will show:

```
Welcome to Microsoft Azure Linux 3.0!
installer-iso-mos login:
```

You're now in the installer environment. Since we removed the automatic
installation service when creating the ISO, Trident will not run automatically,
allowing us to configure and execute the installation manually.

### Step 5: Configure the installation

First, let's identify the target disk for installation:

```bash
lsblk
```

Look for the main disk where you want to install Azure Linux (e.g. `/dev/sda`,
`/dev/vda`, `/dev/nvme0n1`) with at least 16GB of space.

If your selected disk is not `/dev/sda`, update the Trident Host Configuration
to specify the correct disk device:

```bash
# Set your disk device (replace with your actual disk from lsblk output)
DISK_PATH="</dev/your-disk>"  # Change this to your actual disk!

# Update the Host Configuration
sed -i "s|device: /dev/sda|device: ${DISK_PATH}|g" /etc/trident/config.yaml
```

You can verify the change was applied correctly:

```bash
cat /etc/trident/config.yaml
```

This will show the complete Host Configuration. Look for the `device:` line
under the `storage` section to confirm your disk path is correct.

Now start the installation:

```bash
trident install
```

Watch as Trident performs the automated installation process. After 2-3 minutes,
you will see the installation completed successfully and the system will reboot
automatically.

### Step 6: Boot into Azure Linux

After the reboot, we'll see the GRUB bootloader, then Azure Linux starting up.
The installation is complete when you see the login prompt:

```
Welcome to Microsoft Azure Linux 3.0!
testimage login: 
```

**We have successfully installed Azure Linux using Trident!**

Now you can explore your new Azure Linux system.

The system presented a login prompt, but default configuration uses SSH key-only
authentication (no password login available). To explore the system, you can use
the configured SSH access given to the `tutorial-user` (as shown in the `users`
section of the Host Configuration at [Step 5: Create Trident Host Configuration
for
Install](./Building-AB-Update-Images-for-Install-and-Update.md#step-5-create-trident-host-configuration-for-install)).
You can connect from your host machine as follows:

```bash
# From your host machine:
ssh tutorial-user@<system-ip-address>
```

## Appendix: Virtual Machine Setup

For testing purposes, you can use virtual machines to experience Trident's clean
install process. Choose the section that matches your system:

### Option A: Linux with libvirt/KVM

**Set up virtualization environment:**

First, verify that your Linux system supports virtualization:

```bash
sudo apt-get update
sudo apt-get install -y cpu-checker
kvm-ok
```

Install the virtualization tools:

```bash
sudo NEEDRESTART_MODE=a apt-get install -y \
    qemu-kvm \
    libvirt-daemon-system \
    libvirt-clients \
    bridge-utils \
    virt-manager \
    ovmf
```

Add your user to the libvirt group (restart is required afterwards):

```bash
sudo usermod -a -G libvirt $USER
newgrp libvirt
```

Test that libvirt is working:

```bash
virsh list --all
```

You will see an empty list, confirming libvirt is ready.

**Create the virtual machine:**

Prepare installation files and create a disk:

```bash
# Replace '<installer.iso>' with the actual name of your ISO file
sudo cp <installer.iso> /tmp/trident-installer.iso
sudo qemu-img create -f qcow2 /tmp/azure-linux-vm.qcow2 16G
```

Create a simple VM using virt-install:

```bash
sudo virt-install \
  --name azure-linux-vm \
  --ram 4096 \
  --vcpus 2 \
  --disk path=/tmp/azure-linux-vm.qcow2,format=qcow2 \
  --cdrom /tmp/trident-installer.iso \
  --os-type linux \
  --os-variant generic \
  --boot uefi \
  --graphics none \
  --serial pty \
  --console pty,target_type=serial \
  --network network=default \
  --noautoconsole
```

Connect to the VM console:

```bash
sudo virsh console azure-linux-vm
```

**Cleanup when finished:**

```bash
sudo virsh destroy azure-linux-vm
sudo virsh undefine azure-linux-vm
sudo rm /tmp/azure-linux-vm.qcow2 /tmp/trident-installer.iso
```

### Option B: Windows with Hyper-V

**Create the virtual machine:**

1. Open Hyper-V Manager from the Start menu
2. Click **Action** → **New** → **Virtual Machine**
3. Choose **Next** in the wizard
4. Name: `Azure Linux VM`
5. Choose **Generation 2** (UEFI support)
6. Memory: `4096 MB`
7. Network: **Default Switch**
8. Create a new virtual hard disk: `16 GB`
9. **Install from bootable image file**: Browse and select your Servicing ISO
10. Click **Finish**

**Start the VM:**

In Hyper-V Manager, right-click your VM and select **Connect**, then click
**Start**.

**Cleanup when finished:**

In Hyper-V Manager, right-click the VM and select **Delete** to remove it
completely.
