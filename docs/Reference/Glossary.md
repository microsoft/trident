# Glossary

## A/B Update

A [servicing type](#servicing-type) where two copies of the OS are kept on the
device in [A/B Volume Pairs](#ab-volume-pair), and only one is active at a
time. When an update is performed, the inactive copy is updated, and then the
device is rebooted into the updated copy.

## A/B Volume Pair

A pair of [block devices](#block-device) that are used for an [A/B
update](#ab-update). One volume is the 'A' volume, and the other is the 'B'
volume. At any point in time, only one volume is active, and the other is
inactive.

An [A/B Update](#ab-update) is performed by updating the inactive volume, and
then rebooting the device into the updated volume. When this happens, the active
volume swaps from 'A' to 'B', or from 'B' to 'A'.

A system can have multiple A/B volume pairs, each pair representing a different
mount point on the device. All pairs in an [install](#install) are updated in
lockstep, meaning all pairs will have their A volume be the active one, or all
pairs will have their B volume be the active one.

## Block Device

Kernel abstraction generally used for non-volatile storage devices, such as hard
drives, SSDs, and USB drives.

> A file that refers to a device. A block special file is normally distinguished
> from a character special file by providing access to the device in a manner such
> that the hardware characteristics of the device are not visible.
>
> ([Block Special
> File](https://pubs.opengroup.org/onlinepubs/9699919799/basedefs/V1_chap03.html#tag_03_79))

## Clean Install

A [servicing type](#servicing-type) where a new [install](#install) is
performed.

A clean install does not update or modify an existing OS. It creates an entirely
new install on the device.

A clean install is what you do when you install an OS for the first time, or
when you are replacing an existing OS with a new one.

## Dualboot

See [Multiboot](#multiboot).

## Execroot

Execution root. The root file system of the environment where Trident was run.
Generally the Management OS, the OS that is being updated, or a container
running on top of one of the former environments.

## Finalize (Operation)

The final operation performed as part of a [servicing](#servicing). The finalize
operation performs any final steps needed to complete the servicing and have the
host reboot into the new image.

## Install

A full deployment of a Azure Linux made with Trident.

The install encompasses the entire OS, including the bootloader, the kernel, the
initramfs, the root filesystem, all [A/B Volume Pairs](#ab-volume-pair),
associated partitions, and any other partitions that are part of the install.

_Note: This definition does not consider other OSes or distros._

## Multiboot

The capability of having multiple [installs](#install) on the same device, even
on the same disk.

## Newroot

Root file system of the OS that is being deployed.

When Trident is deploying a new OS, it will mount the new OS's file systems and
prepare them for a chroot. This mount of the new OS is called `newroot`.

## Operation

Operations are the top level actions performed inside a [servicing](#servicing).
Trident installations and updates perform the [stage](#stage-operation) and
[finalize](#finalize-operation) operations.

## Servicing

The general process of performing an action on an [install](#install).
There are several [types of servicing](#servicing-type).

## Servicing Type

The specific kind of [Servicing](#servicing) that is being performed on an
install, such as [clean install](#clean-install), or an [A/B
update](#ab-update).

## Stage (Operation)

Stage is an [operation](#operation) that downloads, writes and prepares an OS
image as part of a [servicing](#servicing).

## Step

Steps are logical stages of an operation. On each step, [subsystems](#subsystem)
are run in a specific order to perform the work needed for that step.

## Subsystem

A logical grouping of related functionality within Trident. Each subsystem is in
charge of a specific aspect of the servicing process and configuration of the
[newroot](#newroot).
Subsystems run the corresponding logic for each [step](#step) of an
[operation](#operation).

## Unformatted Partition

An unformatted partition is a partition on a storage device that has been
created but does not yet contain a filesystem. It is not associated with any
filesystem, verity-filesystem, RAID array, or encryption volume.
