
# Perform a Clean Install

## Goals

Use Trident to perform a [clean install](../Reference/Glossary.md#clean-install) of a [target operating system](../Reference/Glossary.md#target-os).

## Instructions

### Step 1: Create Target OS Image

Build a target OS image, i.e. a COSI file. Please reference this [Tutorial
on Building a Deployable Image](../Tutorials/Building-a-Deployable-Image.md).

### Step 2: Create Servicing OS ISO

Build a servicing OS ISO. Please reference this [Tutorial on Building a
Servicing ISO](../Tutorials/Building-a-Servicing-ISO.md) for steps on
how to use Image Customizer to build an installer ISO. This is the ISO from
which the servicing OS will run.

### Step 3: Create Bootable Media

Use the tool of your choice to create bootable media from the servicing OS ISO.

### Step 4: Install Target OS

Ensure that the bootable media is at the top of the boot order using a tool like efibootmgr or select the media during the subsequent boot using the appropriate key (often F12).

When the servicing OS ISO is booted, Trident will stage and boot into the target operating system.
