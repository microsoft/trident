---
sidebar_position: 1
---

# Trident - Azure Linux Servicing Agent

**Trident** is a declarative, security-first OS lifecycle agent designed
primarily for [Azure
Linux](https://github.com/microsoft/azurelinux/?tab=readme-ov-file#azure-linux).
It supports installation and provisioning of bare-metal hosts, as well as
A/B-style atomic updates and runtime configuration for both bare-metal and
virtual machines.

## What can Trident do?

Trident offers a comprehensive set of capabilities for OS installation and
servicing.

**Installation Features:**

- Disk partitioning and formatting using the GUID Partition Table (GPT).
- [Creation of software RAID arrays](How-To-Guides/Create-RAID-Arrays.md),
  [including support for ESP redundancy](How-To-Guides/Set-Up-Redundant-ESP.md).
- [Provisioning of encrypted volumes, with optional PCR
  sealing](How-To-Guides/Create-Encrypted-Volume.md).
- [DM-verity integration for root](How-To-Guides/Set-Up-Root-Verity.md) and
  `/usr` filesystems.
- [Adoption of existing partitions and filesystems
  (preview)](How-To-Guides/Adopt-Existing-Partitions.md).
- Multiboot support for side-by-side installation of multiple OS images
  (preview).

**Installation and Servicing Features:**

- Deployment of compressed, minimized OS images in COSI format from local files,
  HTTPS sources, or OCI registries.
- Bootloader configuration, supporting both `grub2` and `systemd-boot`.
- OS configuration management, including [network
  settings](How-To-Guides/Configure-Networking.md), hostname, [user
  accounts](How-To-Guides/Configure-Users.md), SSH, and SELinux policies.
- [Execution of user-provided scripts for custom OS image modifications](Tutorials/Running-Custom-Scripts.md).
- Reliable rollback to the previous OS version in case of servicing issues.
- Unified Kernel Image (UKI) support (preview).

Trident supports servicing both bare-metal hosts and virtual machines.

Trident runs on both `x86_64` and `aarch64` architectures.

<!-- ## See a prerecorded demo of Trident in action

[![Trident
Demo](https://img.youtube.com/vi/0/0.jpg)](https://www.youtube.com/watch?v=0) -->

## How can I get started?

### Found an issue or missing a feature?

If you found a bug or want to request a feature, please file an issue in the
[Trident GitHub repository](https://github.com/microsoft/trident/issues).

### Try out Trident

#### Do you want to author a sample Host Configuration?

You can start with the [Writing a Simple Host
Configuration](Tutorials/Writing-a-Simple-Host-Configuration.md) tutorial.

#### Do you want to deploy a bare-metal host?

You can start with the [Perform a Clean
Install](How-To-Guides/Perform-a-Clean-Install.md) tutorial.

#### Do you want to make sure the VM image you built with Image Customizer is ready for servicing?

You can start with the [Onboard a VM to
Trident](Tutorials/Onboard-a-VM-to-Trident.md) tutorial.

#### Do you want to update a bare-metal host or a virtual machine?

You can start with the [Performing an A/B
update](Tutorials/Performing-an-ABUpdate.md) tutorial.

<!-- #### Do you want to orchestrate Trident servicing operations across your fleet?

[Get started with orchestration](Trident-Orchestration.md). -->

### Contribute to Trident

Trident is an open source project and we welcome contributions. If you want to
contribute, please check out the [contributing
guide](https://github.com/microsoft/trident/blob/main/CONTRIBUTING.md).

## Do you want to learn more?

- [Motivation](Motivation.md)
- [What is Trident?](What-Is-Trident.md)
- [How does Trident work?](How-Does-Trident-Work.md)
- [How do I interact with Trident?](How-Do-I-Interact-With-Trident.md)
- [Future developments](Future-Developments.md)
