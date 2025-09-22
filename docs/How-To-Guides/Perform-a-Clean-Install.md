
# Perform a Clean Install

<!--
DELETE ME AFTER COMPLETING THE DOCUMENT!
---
Task: https://dev.azure.com/mariner-org/polar/_workitems/edit/13143
Title: Perform a Clean Install
Type: How-To Guide
Objective:

Guide the user through the process of performing a clean install of AzL 3.0 from
an ISO. Refer to guide on creating install media and runtime images.
-->

## Goals

Use Trident to perform a clean installation of a runtime operating system.

## Instructions

### Step 1: Create an Azure Linux image

Create an [Azure Linux image COSI file](../Tutorials/Building-a-Deployable-Image.md)

### Step 2: Create an installation ISO

Create an [installation ISO](../Tutorials/Building-a-Provisioning-ISO.md)

### Step 3: Create bootable media

For bare metal installations, use the tool of your choice to create bootable media from the installation ISO.

For virtual machine installations, the ISO can be used directly.

### Step 4: Boot from media

Ensure that the bootable media is in the boot order or select the boot media during subsequent boot.

When the ISO is booted, Trident will apply the included Host Configuration and COSI file, staging and booting into the desired runtime operating system.  
