# Trident Hello World

In this tutorial, we will install Azure Linux using the Trident installer on a virtual machine. Along the way, we will download the installer ISO (TODO: change to build), create a virtual machine, and complete a clean OS installation. By the end, you will have a running Azure Linux system that you can log into and explore.

## What we'll build

We will create a virtual machine running Azure Linux 3.0, installed using the Trident automated installer. The installation process will:

- Set up disk partitioning and filesystems automatically
- Install a complete Azure Linux system with Trident management capabilities
- Configure the system for first boot
- Provide you with a working Linux environment

## Prerequisites

Before we start, you'll need:

- A Linux system with libvirt/KVM support (Ubuntu 20.04+ recommended)
- At least 16GB of available disk space
- Azure CLI installed and configured
- Administrative (sudo) access

Let's check that your system has virtualization support:

```bash
sudo apt-get update
sudo apt-get install -y cpu-checker
kvm-ok
```

You should see output indicating that KVM acceleration can be used. If not, you can still follow this tutorial, but the VM will run more slowly.

## Step 1: Download the Azure Linux installer

First, we need to download the Trident installer ISO from the Azure DevOps artifacts feed.

Log into Azure using the Azure CLI:

```bash
az login
```

This will open a browser window where you can authenticate. After successful login, you'll see output showing your account details.

Now download the installer ISO:

```bash
az artifacts universal download \
  --organization "https://dev.azure.com/mariner-org/" \
  --project "2311650c-e79e-4301-b4d2-96543fdd84ff" \
  --scope project \
  --feed "Trident" \
  --name "usb-iso" \
  --version "0.3.2025090401-v986e79e" \
  --path .
```

You should see a file named `azl-installer.iso`. This is our installer image that contains both the live boot environment and the Azure Linux system to be installed.

## TODO: Add Hyper-V flow 
TODO: Fix with according disk path for the tutorial.
Q: Why is installation running from the begining? 

libvirt following pipeline:

## Step 2: Install virtualization tools

We need to install the tools required to create and manage virtual machines:

```bash
sudo NEEDRESTART_MODE=a apt-get install -y \
    qemu-kvm \
    libvirt-daemon-system \
    libvirt-clients \
    bridge-utils \
    virt-manager \
    ovmf
```

Add your user to the libvirt group so you can manage VMs without sudo:

```bash
sudo usermod -a -G libvirt $USER
```

After running this command, you'll need to log out and log back in for the group change to take effect. You can also start a new shell session:

```bash
newgrp libvirt
```

Configure libvirt to use the system connection by default:

```bash
mkdir -p ~/.config/libvirt
cat << EOF > ~/.config/libvirt/libvirt.conf
uri_default = "qemu:///system"
EOF
```

Let's verify that libvirt is working:

```bash
virsh list --all
```

You should see an empty list of virtual machines, which confirms that libvirt is running correctly.

## Step 3: Prepare the VM environment

First, we'll copy our ISO to a standard location and create a disk for our VM:

```bash
sudo cp azl-installer.iso /tmp/azl-installer.iso
sudo qemu-img create -f qcow2 /tmp/azure-linux-vm.qcow2 16G
```

Notice that we're creating a 16GB disk image. This provides enough space for the Azure Linux installation plus some room for your own use.

Now create a network configuration file for our VM. This will provide isolated networking:

```bash
cat << EOF > vm-network.xml
<network>
    <name>azure-linux-vm-net</name>
    <forward mode="route" />
    <domain name="azure-linux-vm-net" />
    <ip address='192.168.242.1' netmask='255.255.255.0'>
      <dhcp>
        <range start='192.168.242.10' end='192.168.242.50'/>
        <host mac='52:54:00:12:34:56' name='azure-linux-vm' ip='192.168.242.10'/>
      </dhcp>
    </ip>
</network>
EOF
```

Create the network:

```bash
sudo virsh net-create vm-network.xml
```

You should see output confirming that the network was created successfully.

## Step 4: Create the virtual machine

Now we'll create the VM definition. This tells libvirt how to configure our virtual machine:

```bash
cat << EOF > azure-linux-vm.xml
<domain type="kvm">
  <name>azure-linux-vm</name>
  <memory unit="KiB">4194304</memory>
  <currentMemory unit="KiB">4194304</currentMemory>
  <vcpu placement="static">2</vcpu>
  <os>
    <type arch="x86_64" machine="pc-q35-6.2">hvm</type>
    <loader readonly="yes" type="pflash">/usr/share/OVMF/OVMF_CODE_4M.fd</loader>
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
      <source file="/tmp/azl-installer.iso" />
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
      <source network="azure-linux-vm-net" />
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

Register the VM with libvirt:

```bash
sudo virsh define azure-linux-vm.xml
```

Let's verify our VM was created:

```bash
virsh list --all
```

You should now see `azure-linux-vm` listed as a defined but not running virtual machine.

## Step 5: Start the installation

Now we're ready to boot our VM from the installer ISO:

```bash
sudo virsh start azure-linux-vm
```

The VM will start booting from the ISO. Connect to the console to watch the installation:

```bash
sudo virsh console azure-linux-vm
```

You will see the boot process start. After a few moments, you should see output similar to:

```
Welcome to Azure Linux 3.0!
azl-installer-mos login: root (automatic login)
```

This confirms that the installer environment has booted successfully and automatically logged you in as root.

## Step 6: Configure and run the installation

The installer uses a configuration file that tells it how to set up the target system. By default, it expects to install to `/dev/nvme0n1`, but our VM uses `/dev/sda`. Let's update the configuration:

```bash
sed -i 's|device: /dev/nvme0n1|device: /dev/sda|' /etc/trident/config.yaml
```

Let's check that the target disk is available:

```bash
lsblk
```

You should see output showing `/dev/sda` as a 16GB disk, which is our target installation disk.

Now start the installation:

```bash
trident install
```

The installation will begin automatically. You will see progress messages as Trident:

1. Partitions the target disk
2. Creates filesystems
3. Extracts and installs the Azure Linux system
4. Configures the bootloader
5. Sets up the system for first boot

The installation typically takes 2-3 minutes. You'll see output showing each step of the process. When complete, you should see:

```
Installation completed successfully!
```

The system will then automatically reboot.

## Step 7: Boot into your new Azure Linux system

After the reboot, the system will boot from the hard disk instead of the ISO. You will see the GRUB bootloader, followed by the Azure Linux boot process.

Eventually, you should see:

```
trident-testimg login:
```

This is your new Azure Linux system! Log in using the default credentials:

- Username: `root`
- Password: `p@ssw0rd`

```bash
root
p@ssw0rd
```

After logging in, you'll see a command prompt:

```bash
[root@trident-testimg ~]#
```

Congratulations! You now have a running Azure Linux system installed via Trident.

## Step 8: Explore your new system

Let's verify what we've installed. Check the operating system version:

```bash
cat /etc/os-release
```

You should see output confirming this is Azure Linux 3.0.

Check the disk layout that Trident created:

```bash
lsblk
df -h
```

Notice that Trident has set up multiple partitions including boot, root filesystem, and swap space.

Check that Trident services are running:

```bash
systemctl status trident-service
```

You should see that the trident-service is active and running.

Finally, check network connectivity:

```bash
ip addr show
ping -c 3 8.8.8.8
```

You should see that your VM has received an IP address and can reach the internet.

## What you've accomplished

You have successfully:

- Downloaded the Azure Linux installer ISO from the Azure DevOps artifacts feed
- Set up a complete virtualization environment using libvirt and KVM
- Created and configured a virtual machine with appropriate hardware settings
- Booted the Trident installer and performed an automated OS installation
- Configured the installation for your specific hardware (changing from NVMe to SATA)
- Completed a full Azure Linux installation with automatic partitioning and system setup
- Booted into your new system and verified its functionality

You now have a working Azure Linux 3.0 system managed by Trident that you can use for further exploration and development.

## Cleanup (optional)

When you're done experimenting, to exit the VM console, press `Ctrl+]`.

You can clean up the VM and resources:

```bash
# Stop and remove the VM
sudo virsh destroy azure-linux-vm
sudo virsh undefine azure-linux-vm

# Remove the network
sudo virsh net-destroy azure-linux-vm-net

# Remove files
sudo rm /tmp/azure-linux-vm.qcow2
sudo rm /tmp/azl-installer.iso
rm azure-linux-vm.xml
rm vm-network.xml
```

