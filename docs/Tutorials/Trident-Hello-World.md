# Trident Hello World

In this tutorial, we will install Azure Linux on a virtual machine using Trident. We'll create a virtual machine, boot from a [Provisioning ISO](./Building-a-Provisioning-ISO.md), and install Azure Linux using Trident.

## Introduction

We will create a complete Azure Linux system using Trident. You will see how Trident transforms a blank virtual machine into a fully configured Azure Linux system in just a few minutes.

## Prerequisites

Before we start, you'll need:

1. **A Trident Provisioning ISO**
   - Follow the [Building a Provisioning ISO](./Building-a-Provisioning-ISO.md) guide to create your installer.

2. **Choose one virtualization platform:**
   - **Linux system** with libvirt/KVM support, OR  
   - **Windows system** with Hyper-V enabled

3. **System resources**
   1. At least 16GB of available disk space.
   2. 4GB of available RAM.
   3. Administrative access (sudo on Linux).

## Instructions

Choose the section that matches your system:

- **Linux with libvirt/KVM** - Follow Section A
- **Windows with Hyper-V** - Follow Section B

After completing your section, continue to "Start the Installation"

## Section A: Linux with libvirt/KVM

### Step 1: Set up virtualization environment

First, let's verify that your Linux system supports virtualization:

```bash
sudo apt-get update
sudo apt-get install -y cpu-checker
kvm-ok
```

You will see output confirming that KVM acceleration is available.

Now we'll install the virtualization tools:

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

Configure libvirt:

```bash
mkdir -p ~/.config/libvirt
cat << EOF > ~/.config/libvirt/libvirt.conf
uri_default = "qemu:///system"
EOF
```

Test that libvirt is working:

```bash
virsh list --all
```

You will see an empty list, confirming libvirt is ready.

### Step 2: Create the virtual machine

First, we'll prepare our installation files. Copy your Provisioning ISO and create a disk:

```bash
# Replace 'trident-provisioning.iso' with the actual name of your ISO file
sudo cp trident-provisioning.iso /tmp/trident-installer.iso
sudo qemu-img create -f qcow2 /tmp/azure-linux-vm.qcow2 16G
```

You now have a 16GB virtual disk for Azure Linux.

Create the VM configuration:

```bash
cat << EOF > azure-linux-vm.xml
<domain type="kvm">
  <name>azure-linux-vm</name>
  <memory unit="KiB">4194304</memory>
  <currentMemory unit="KiB">4194304</currentMemory>
  <vcpu placement="static">2</vcpu>
  <os>
    <type arch="x86_64" machine="pc-q35-6.2">hvm</type>
    <loader readonly="yes" type="pflash">/usr/share/OVMF/OVMF_CODE.fd</loader>
    <boot dev="cdrom"/>
    <boot dev="hd"/>
  </os>
  <features>
    <acpi />
    <apic />
    <vmport state="off" />
  </features>
  <cpu mode="custom" match="exact" check="none">
    <model fallback="allow">Broadwell-IBRS</model>
  </cpu>
  <clock offset="utc">
    <timer name="rtc" tickpolicy="catchup" />
    <timer name="pit" tickpolicy="delay" />
    <timer name="hpet" present="no" />
  </clock>
  <on_poweroff>destroy</on_poweroff>
  <on_reboot>restart</on_reboot>
  <on_crash>destroy</on_crash>
  <devices>
    <emulator>/usr/bin/qemu-system-x86_64</emulator>
    <disk type="file" device="disk">
      <driver name="qemu" type="qcow2" />
      <source file="/tmp/azure-linux-vm.qcow2" />
      <target dev="sda" bus="sata" />
      <address type="drive" controller="0" bus="0" target="0" unit="1" />
    </disk>
    <disk type="file" device="cdrom">
      <driver name="qemu" type="raw" />
      <source file="/tmp/trident-installer.iso" />
      <target dev="sdx" bus="sata" />
      <readonly />
      <address type="drive" controller="0" bus="0" target="0" unit="0" />
    </disk>
    <controller type="virtio-serial" index="0">
      <address type="pci" domain="0x0000" bus="0x04" slot="0x00" function="0x0" />
    </controller>
    <serial type="pty">
      <target type="isa-serial" port="0">
        <model name="isa-serial" />
      </target>
    </serial>
    <interface type="network">
      <mac address="52:54:00:12:34:56" />
      <source network="default" />
      <model type="virtio" />
      <address type="pci" domain="0x0000" bus="0x02" slot="0x00" function="0x0" />
    </interface>
    <console type="pty">
      <target type="serial" port="0" />
    </console>
    <memballoon model="virtio">
      <address type="pci" domain="0x0000" bus="0x05" slot="0x00" function="0x0" />
    </memballoon>
  </devices>
</domain>
EOF
```

Register the VM:

```bash
sudo virsh define azure-linux-vm.xml
```

Verify the VM was created:

```bash
virsh list --all
```

You will see `azure-linux-vm` in the list, ready to start.

### Step 3: Start the VM

Start the VM:

```bash
sudo virsh start azure-linux-vm
```

Connect to the console:

```bash
sudo virsh console azure-linux-vm
```

You will see the boot process begin and eventually reach the installer environment.

## Section B: Windows with Hyper-V

### Step 1: Create the virtual machine

Open Hyper-V Manager from the Start menu.

Create a new virtual machine:

1. Click **Action** → **New** → **Virtual Machine**
2. Choose **Next** in the wizard
3. Name: `Azure Linux VM`
4. Choose **Generation 2** (UEFI support)
5. Memory: `4096 MB`
6. Network: **Default Switch**
7. Create a new virtual hard disk: `16 GB`
8. **Install from bootable image file**: Browse and select your Provisioning ISO
9. Click **Finish**

### Step 2: Start the VM

In Hyper-V Manager, right-click your VM and select **Connect**, then click **Start**.

You will see the boot process begin and eventually reach the installer environment.

## Start the Installation

**Both platforms:**

After a few moments, the screen will show:

```
Welcome to Azure Linux 3.0!
azl-installer login: root 
```

### Configure the installation

First, let's see what disk Trident will install to:

```bash
lsblk
```

You will see `/dev/sda` or similat as a 16GB disk. This is our installation target.

We need to update the Trident configuration for our VM's disk.

**Using vim see/modify the Host Configuration:**

```bash
vim /etc/trident/config.yaml
```

In vim, find the line with `device: /dev/nvme0n1` and change it to the correct device, for example: `device: /dev/sda`; then save and exit (`:wq`).

Now start the installation:

```bash
trident install
```

Watch as Trident performs the installation.

After 2-3 minutes, you will see:

```
Installation completed successfully!
```

The system will reboot automatically.

### Boot into Azure Linux

After the reboot, we'll see the GRUB bootloader, then Azure Linux starting up.
You can verify the system is running by checking the console output shows Azure Linux has booted successfully.

The login prompt appears:

```
trident-testimg login:
```

The system will present a login prompt. Since the default configuration uses SSH key-only authentication (no password login), you can access the system via SSH once the installation is complete.

We now have Azure Linux running!

### Explore your system

If you have SSH access configured (with SSH keys in your Provisioning ISO configuration), you can connect to explore the system:

```bash
# From your host machine, if SSH is configured:
ssh testing-user@<vm-ip-address>
```

We have successfully created a complete Azure Linux system using Trident!
Now you can explore your newly Azure Linux system.

**Cleanup (Linux users):**

When finished, exit the console with `Ctrl+]`, then clean up:

```bash
sudo virsh destroy azure-linux-vm
sudo virsh undefine azure-linux-vm
sudo rm /tmp/azure-linux-vm.qcow2 /tmp/trident-installer.iso
rm azure-linux-vm.xml
```

**Cleanup (Windows users):**

In Hyper-V Manager, right-click the VM and select **Delete** to remove it completely.
