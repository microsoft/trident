# Trident Hello World

In this tutorial, we will install Azure Linux on a virtual machine using Trident. Trident is designed to perform clean installs on bare metal hosts, but for demonstration purposes, a virtual machine can also be used. We'll boot from the [Provisioning ISO](./Building-a-Provisioning-ISO.md), and use Trident to install and configure Azure Linux.

## Introduction

We will create a complete Azure Linux system using Trident. You will see how Trident transforms a blank virtual machine into a fully configured Azure Linux system in just a few minutes!!!

## Prerequisites

Before we start, you'll need:

1. **Provisioning ISO**
   - Follow the [Building a Provisioning ISO](./Building-a-Provisioning-ISO.md) guide to create your installer

2. **A test target system**
   - Either a physical machine for bare-metal installation, OR
   - A virtual machine for testing (see [Appendix: Virtual Machine Setup](#appendix-virtual-machine-setup))

3. **System resources**
   - At least 16GB of available disk space on the target system
   - 4GB of available RAM
   - Administrative access

## Instructions

### Step 1: Boot from the Provisioning ISO

**Create Provisioning ISO**
Follow the [Building a Provisioning ISO](./Building-a-Provisioning-ISO.md) tutorial and use the tool of your choice to create bootable media from it.

Insert the bootable media (USB drive or CD/DVD) into your target system and power it on. Make sure to configure it to boot from the media first or select the media during the subsequent boot using the appropriate key (often F12).

The system will boot into the Azure Linux installer environment.

### Step 2: Access the installer environment

After a few moments, the screen will show:

```
Welcome to Azure Linux 3.0!
azl-installer login: root 
```

You're now in the installer environment, ready to configure and run the installation.

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

Watch as Trident performs the automated installation process. After 2-3 minutes, you will see:

```
Installation completed successfully!
```

The system will reboot automatically.

### Step 4: Boot into Azure Linux

After the reboot, we'll see the GRUB bootloader, then Azure Linux starting up.
The installation is complete when you see the login prompt:

```
trident-testimg login:
```

The system will present a login prompt. Since the default configuration uses SSH key-only authentication (no password login), you can access the system via SSH once the installation is complete.

We now have Azure Linux running!

### Step 5: Explore your system

If you have SSH access configured (with SSH keys in your Provisioning ISO configuration), you can connect to explore the system:

```bash
# From your host machine, if SSH is configured:
ssh testing-user@<system-ip-address>
```

**We have successfully created a complete Azure Linux system using Trident!**
Now you can explore your new Azure Linux system.

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
# Replace '<provisioning.iso>' with the actual name of your ISO file
sudo cp <provisioning.iso> /tmp/trident-installer.iso
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
9. **Install from bootable image file**: Browse and select your Provisioning ISO
10. Click **Finish**

**Start the VM:**

In Hyper-V Manager, right-click your VM and select **Connect**, then click **Start**.

**Cleanup when finished:**

In Hyper-V Manager, right-click the VM and select **Delete** to remove it completely.
