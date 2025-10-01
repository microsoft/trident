
# Create a RAID Array

This guide explains how to create a new [software RAID
array](../Reference/Host-Configuration/API-Reference/SoftwareRaidArray.md) on
[clean install](../Reference/Glossary.md#clean-install) with Trident, using the
Host Configuration API.

## Goals

By following this guide, you will:

1. Declare RAID arrays using the Host Configuration API.
1. Configure RAID arrays to be mounted at specified mount points in the target
   OS.
1. Set an optional timeout so that Trident waits for RAID arrays to finish
   syncing before continuing with provisioning.
1. Create RAID arrays on the target OS with Trident.

This guide will not cover adopting an existing software RAID array in the
[`offline-init`](../Explanation/Offline-Init.md) scenario or creating a new
software RAID array on A/B updates, as Trident does **not** support these
features.

## Prerequisites

1. A host that has not yet been serviced by Trident.
1. A Host Configuration with the basic structure, including the
   [`storage`](../Reference/Host-Configuration/API-Reference/Storage.md)
   section.

## Instructions

### Step 1: Declare Devices Underlying the RAID Array

1. Declare devices to be used for the RAID array using the Host Configuration
   API. [The RAID wiki](https://wiki.archlinux.org/title/RAID) contains more
   information on RAID arrays, including how many devices are needed. These
   requirements differ for different RAID levels; however, Trident only tests
   RAID 1. For RAID 1, devices underlying a RAID array must adhere to the
   following guidelines:

   - Any disk partition types are allowed, but all underlying devices must be of
     the same partition type.
   - All underlying disk partitions must be of the same size.
   - Each RAID array should be based on 2+ disk partitions that are located on
     different disks.

### Step 2: Add `raid` Configuration

1. Inside the `storage` config, add a new software RAID to [the `raid`
   configuration](../Reference/Host-Configuration/API-Reference/Raid.md),
   completing these four **required** fields:

   - `devices` is a list of block device IDs corresponding to the disk
     partitions underlying the RAID array.
   - `id` is the unique identifier for the RAID array. The ID must be unique
     across all types of devices in the Host Configuration.
   - `level` is the RAID level. Only `raid1` is supported and tested. Other
     possible values yet to be tested are: `raid0`, `raid5`, `raid6`, `raid10`.
   - `name` is the name of the RAID array that will be used to reference the
     RAID array on the system. For example, `some-raid` will be accessible as
     `/dev/md/some-raid` on the system.

   For example, the following configuration describes a layout where there are
   two disks, `disk1` and `disk2`. Each disk contains a `root` partition of the
   same size (4G). Then, in the `raid` config, a new `raid1` RAID array with ID
   `root` and name `root` is described. It is based on the mirrored partitions:
   `root1` and `root2`.

   ```yaml
   storage:
     disks:
       - id: disk1
         device: /dev/disk/by-path/disk1
         partitionTableType: gpt
         partitions:
           - id: esp1
             type: esp
             size: 100M
           - id: root1
             type: root
             size: 4G
           - id: trident1
             type: linux-generic
             size: 500M
       - id: disk2
         device: /dev/disk/by-path/disk2
         partitionTableType: gpt
         partitions:
           - id: root2
             type: root
             size: 4G
     raid:
       software:
         - id: root
           name: root
           level: raid1
           devices:
             - root1
             - root2
   ```

   **Naming Convention**: In Trident, it is conventional to suffix the ID of the
   RAID array with digit indices to create the partition device IDs. For
   instance, two disk partitions `trident1` and `trident2` would underlie a RAID
   array with ID `trident`. It is also conventional to use the same short,
   descriptive string for both the `id` and the `name`.

1. If the RAID array needs to be mounted, the `storage.filesystems`
   configuration must be updated accordingly:

   ```yaml
   storage:
     filesystems:
       - deviceId: root
         mountPoint: /
   ```

   For example, this configuration specifies that the RAID array with ID `root`
   from above should be mounted at `/`, using the filesystem provided in the
   Host Configuration. [The API documentation on
   filesystems](../Reference/Host-Configuration/API-Reference/FileSystem.md)
   contains more information on the filesystems configuration.

1. The `raid` configuration also has an optional field called `syncTimeout` that
   applies to all RAID arrays created with Trident. `syncTimeout` is the timeout
   in seconds to wait for RAID arrays to sync.

   By default, Trident will **not** wait for RAID arrays to finish syncing
   before continuing with provisioning. This is because RAID arrays are supposed
   to be usable immediately after creation. If you provide a value for this
   field and the RAID arrays do **not** finish syncing within the specified
   timeout, Trident will fail the provisioning process and return an error. You
   will need to increase the timeout value if the RAID arrays are taking longer
   to sync than expected.

   For example, the following configuration establishes a 10-second timeout to
   wait for RAID arrays to sync:

   ```yaml
   storage:
     raid:
       software:
         - id: root
           name: root
           level: raid1
           devices:
             - root1
             - root2
       syncTimeout: 10
   ```

### Step 3: Run Trident to Create RAID Arrays

1. [Run `trident install`](./Perform-a-Clean-Install.md) to create the software
   RAID array on clean install. Trident will:

   - Stop and unmount all existing RAID arrays that are on the disks declared in
     the Host Configuration.
   - Create the RAID arrays declared in the Host Configuration via the `mdadm`
     package, and mount them if requested in the `storage.filesystems`
     configuration.

   To learn more about `mdadm`, please refer to the [mdadm
   guide](https://raid.wiki.kernel.org/index.php/A_guide_to_mdadm).
