
# Perform a Clean Install

## Goals

Use Trident to perform a [clean install](../Reference/Glossary.md#clean-install) of a [target operating system](../Reference/Glossary.md#target-os).

## Instructions

### Step 1: Create Target OS Image

Build a target OS image, i.e. a COSI file. Reference this [Tutorial on Building A/B Update Images for Install and Update](../Tutorials/Building-AB-Update-Images-for-Install-and-Update.md).

### Step 2: Create Servicing OS ISO

Build a servicing OS ISO. Reference this [Tutorial on Building a Servicing ISO](../Tutorials/Building-a-Servicing-ISO.md) for steps on how to use [Image Customizer](https://microsoft.github.io/azure-linux-image-tools/imagecustomizer/README.html) to build an installer ISO. This is the ISO from which the servicing OS will run.

### Step 3: Create Bootable Media

Use the tool of your choice to create bootable media from the servicing OS ISO.

### Step 4: Install Target OS

Ensure that the bootable media is at the top of the boot order using a tool like `efibootmgr` or select the media during the subsequent boot using the appropriate key (often F12).

When the servicing OS ISO is booted, Trident will stage and boot into the target operating system.

### Optional Step: Simulate with a Virtual Machine

The servicing ISO can be validated using a virtual machine to simulate a clean install. To do so, create a virtual machine with an empty disk (this can be created using something like: `qemu-img create -f qcow2 osdisk.qcow2 10G`) and mount the servicing ISO as a CD. When the virtual machine starts, it will boot into the servicing ISO and invoke `trident install`.

> Ensure that the Host Configuration file in the servicing ISO references the virtual machine's disk properly (i.e. for a sata disk, use something like `/dev/sda`).
