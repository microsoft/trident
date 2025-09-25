# Background

Linux distributions typically provide two primary types of bootable images: an
installer image and a virtual machine (VM) image.

## Installer image vs. VM image

The installer image, typically distributed in ISO format, boots into a live
servicing operating system and can be executed in either attended or unattended
modes. During installation, users are able to configure disk partitioning,
select desired features, and set system parameters such as timezone and user
accounts. Behind the scenes, the installer automates essential tasks including
disk partitioning and formatting, package installation, and operating system
configuration. Upon completion, the installer reboots into the fully provisioned
runtime environment. This installation process is suitable for deploying Linux
distributions to both bare-metal hosts and virtual machines.

Alternatively, if your goal is to run the Linux distribution within a virtual
machine, you can obtain a VM image and initiate the boot process immediately.
This approach enables rapid deployment and operation of the Linux environment.
However, configuration changes can only be applied after the initial
boot—typically using tools such as `cloud-init`—which may necessitate additional
reboots to fully realize the desired system state.

## The need for ongoing servicing

Regardless of the image type selected, ongoing servicing is essential to address
security vulnerabilities (CVEs) and apply updates. For environments with spare
resources, scale-out servicing can be performed by deploying a new OS version
onto a separate node—virtual or bare-metal—and decommissioning the older
instance upon completion. However, this approach may not be feasible for larger
clusters or resource-constrained scenarios where spare capacity is unavailable
or cost-prohibitive.

In such cases, servicing can be achieved by shutting down the current OS
instance, replacing the OS disk (either by deploying a new VM image or rerunning
the installer), and booting into the updated OS. This process is time-consuming
and requires additional orchestration from the underlying infrastructure.

## The advantages of in-place servicing

To minimize downtime and avoid reliance on spare resources, in-place servicing
is preferable. Traditional Linux distributions typically support package-based
updates, but these methods lack robust rollback capabilities and can result in
inconsistencies across nodes due to timing and package variations.

A more reliable approach is to use image-based A/B style in-place atomic
updates, similar to those used by Android. With [A/B
updates](Reference/Glossary.md#ab-update), rollback is straightforward—either
during servicing or at any later point—without requiring extra resources.
Additionally, servicing downtime is reduced, as the B set of images can be
pre-staged while the A set remains operational.

## The Trident solution

Traditionally, Linux distributions provide distinct mechanisms for initial
installation and subsequent servicing of the operating system. Trident
streamlines this process by offering a unified workflow that seamlessly handles
both installation and ongoing servicing tasks.

Regardless of whether you are deploying to bare-metal hosts or virtual machines,
and whether you utilize installer images or VM images, Trident delivers a
consistent, atomic approach to OS deployment and servicing. Its composable
architecture enables easy integration into broader solutions, eliminating the
need to manually coordinate low-level OS utilities for disk partitioning, image
installation, bootloader configuration, and system setup—Trident manages these
operations efficiently on your behalf.
