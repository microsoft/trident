
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

Ensure that the bootable media is in the boot order or select the boot media during subsequent boot.  For example, if a bootable USB ISO was created in [step 3](#step-3-create-bootable-media), the boot order could be modified with efibootmgr or, commonly, using F12 (often) during boot.

When the management OS ISO is booted, Trident will apply the included Host Configuration and COSI file, staging and booting into the desired runtime operating system.
