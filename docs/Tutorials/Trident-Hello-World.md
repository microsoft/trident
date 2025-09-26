# Trident Hello World

In this tutorial, we will install Azure Linux on a virtual machine using Trident. Trident is designed to perform clean installs on bare-metal hosts, but for learning/demonstration purposes, a virtual machine can also be used. We'll boot from the [Servicing ISO](./Building-a-Servicing-ISO.md), and use Trident to install and configure Azure Linux.

## Introduction

In this tutorial, we will install Azure Linux using Trident. Trident is a declarative OS lifecycle agent. You will see firsthand how Trident transforms a blank virtual machine into a fully configured Azure Linux system in just a few minutes!

## Prerequisites

Before we start, you'll need:

1. **Servicing ISO**
   - Follow the [Building a Servicing ISO](./Building-a-Servicing-ISO.md) guide to create your installer
   - **Important**: For this tutorial, we need to modify the ISO creation process to disable automatic installation. This allows us to:
     - Select the specific disk for installation.
     - Observe the Host Configuration.
     - Execute the installation ourselves.
   - **Modification required**: In Step 3 of the "Building a Servicing ISO" tutorial, remove the `trident-install.service` from the `services` section in `ic-config.yaml`:
     ```yaml
     services:
       enable:
         - trident-install.service  # <-- Remove this line
         - trident-network.service
     ```
   - This prevents automatically running Trident when the ISO boots, so we can do it ourselves.

2. **A test target system**
   - Either a physical machine for bare-metal installation, OR
   - A virtual machine for testing (see [Appendix: Virtual Machine Setup](#appendix-virtual-machine-setup))

3. **System resources**
   - At least 16GB of available disk space on the target system
   - 4GB of available RAM
   - Administrative access

## Instructions

### Step 1: Boot from the Servicing ISO

**Create Servicing ISO**
Follow the [Building a Servicing ISO](./Building-a-Servicing-ISO.md) tutorial (remember to remove the `trident-install.service` line from the Image Customizer configuration, as described in the Prerequisites) and use the tool of your choice to create bootable media from it.

Insert the bootable media (USB, CD, etc.) into your target system and power it on. Make sure to configure it to boot from the media first or select the media during the subsequent boot using the appropriate key (often F12).

The system will boot into the Azure Linux installer environment.

### Step 2: Access the installer environment

After a few moments, the screen will show:

```
Welcome to Azure Linux 3.0!
installer-iso-mos login: root 
```

You're now in the installer environment. Since we removed the automatic installation service when creating the ISO, Trident will not run automatically, allowing us to configure and execute the installation manually.

### Step 3: Configure the installation

First, let's see what disk Trident will install to:

```bash
lsblk
```

You will see something similar to `/dev/sda` for the target disk. This is where Azure Linux will be installed.

We need to update the Trident Host Configuration to specify the correct disk device.

**Using vim to see/modify the Host Configuration:**

```bash
vim /etc/trident/config.yaml
```

In vim, find the line with `device: <disk>` and change it to the correct device, for example: `device: /dev/sda`; then save and exit (`:wq`).

Now start the installation:

```bash
trident install
```

Watch as Trident performs the automated installation process. After 2-3 minutes, you will see the installation completed successfully and the system will reboot automatically.

### Step 4: Boot into Azure Linux

After the reboot, we'll see the GRUB bootloader, then Azure Linux starting up.
The installation is complete when you see the login prompt.

**We have successfully created a complete Azure Linux system using Trident!**
Now you can explore your new Azure Linux system.

The system will present a login prompt. Default configuration uses SSH key-only authentication (no password login). If you have SSH access configured for your user, you can connect to explore the system:

```bash
# From your host machine, if SSH is configured:
ssh <user>@<system-ip-address>
```

## Appendix: Virtual Machine Setup

For testing purposes, you can use virtual machines to experience Trident's clean install process. Choose the section that matches your system:

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
# Replace '<servicing.iso>' with the actual name of your ISO file
sudo cp <servicing.iso> /tmp/trident-installer.iso
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

In Hyper-V Manager, right-click your VM and select **Connect**, then click **Start**.

**Cleanup when finished:**

In Hyper-V Manager, right-click the VM and select **Delete** to remove it completely.
