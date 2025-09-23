
# Perform a Clean Install

## Goals

Use Trident to perform a [clean install](../Reference/Glossary.md#clean-install) of a [runtime operating system](../Reference/Glossary.md#runtime-os).

## Instructions

### Step 1: Create Runtime OS Image

Build a runtime OS image, i.e. a COSI file. Please reference this [Tutorial
on Building a Deployable Image](../Tutorials/Building-a-Deployable-Image.md).

### Step 2: Create Management OS ISO

Build a management OS ISO. Please reference this [Tutorial on Building a
Provisioning ISO](../Tutorials/Building-a-Provisioning-ISO.md) for steps on
how to use Image Customizer to build an installer ISO. This is the ISO from
which the management OS will run.

### Step 3: Create Bootable Media

Use the tool of your choice to create bootable media from the management OS ISO.

### Step 4: Install Runtime OS

Ensure that the bootable media is at the top of the boot order using a tool like efibootmgr or select the media during the subsequent boot using the appropriate key (often F12).

When the management OS ISO is booted, Trident will stage and boot into the desired runtime operating system.
