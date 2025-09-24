
# Configure an A/B Update Ready Host

This guide explains how to configure the host to be ready for [A/B updates](../Reference/Glossary.md#ab-update), using the Host Configuration API.

## Goals

By following this guide, you will:

1. Declare A/B volume pairs on top of other devices using the Host Configuration.
1. Configure a host so that Trident can service it with A/B updates.

## Prerequisites

1. A host that has not yet been serviced by Trident.
1. A host configuration with the basic structure, including the [`storage`](../Reference/Host-Configuration/API-Reference/Storage.md) section. The configuration should contain A and B copies of a device that will be targeted with an A/B update.
1. A target OS image, i.e. a COSI file, which can be built by referencing this [tutorial](../Tutorials/Building-a-Deployable-Image.md), for [a clean install](../Reference/Glossary.md#clean-install). OR, a VM with an A/B disk layout that can be adopted via [`offline-init`](../Explanation/Offline-Init.md).

## Instructions

### Step 1: Add `abUpdate` configuration

1. Add a `storage.abUpdate` configuration to the host configuration. The [`abUpdate`](../Reference/Host-Configuration/API-Reference/AbUpdate.md) configuration carries information about the [A/B volume pairs](../Reference/Glossary.md#ab-volume-pair) that are used to perform A/B updates.

1. In the `abUpdate` configuration, add `volumePairs`. There can be multiple A/B volume pairs, as long as they are mounted at different mount points. This is a list of A/B volume pairs that will be targeted by A/B updates. Each A/B volume pair consists of two devices, A and B, that have the same type and size and are located in the same disk.

1. Add A/B volume pairs to [`volumePairs`](../Reference/Host-Configuration/API-Reference/AbVolumePair.md). Each A/B volume pair added to `volumePairs` must contain the following three **required** fields:

   - `id` is a unique identifier for the A/B volume pair. This is a user-defined string that links the A/B volume pair to the results in the Host Status and to the [`filesystems`](../Reference/Host-Configuration/API-Reference/FileSystem.md) configuration. The identifier needs to be unique across devices of all types, not just A/B volume pairs.
   - `volumeAId` is the id of the device that will be used as the A volume.
   - `volumeBId` is the id of the device that will be used as the B volume.

   `volumeAId` and `volumeBId` must correspond to two devices that are declared in the same host configuration, following these rules:

   - A/B volumes must be disk partitions of any type, [RAID arrays](../Reference/Host-Configuration/API-Reference/Raid.md), or [encrypted volumes](../Reference/Host-Configuration/API-Reference/EncryptedVolume.md).
   - Each A/B volume pair must consist of exactly two devices of the same type.
   - Both volumes in an A/B volume pair must have the same size.

   For example, the host configuration below declares one A/B volume pair with id `root`. This A/B volume pair consists of two volumes, `root-a` and `root-b`, that are disk partitions. They have the same partition type `root` and are of the same size (8G). Because the `root` A/B volume pair needs to be mounted, the `filesystems` configuration lists `root` with the mount point at `/`.

   ```yaml
   storage:
     disks:
       - id: disk1
         device: /dev/disk/by-path/disk1
         partitionTableType: gpt
         partitions:
           - id: root-a
             type: root
             size: 8G
           - id: root-b
             type: root
             size: 8G
           - id: esp
             type: esp
             size: 1G
     abUpdate:
       volumePairs:
         - id: root
           volumeAId: root-a
           volumeBId: root-b
     filesystems:
       - deviceId: esp
         mountPoint:
           path: /boot/efi
           options: umask=0077
       - deviceId: root
         mountPoint: /
   ```

   **Naming Convention**: In Trident, it is conventional to choose a short, descriptive string as the id for an A/B volume pair. Then, to create the ids for the A/B volumes inside the pair, the id is suffixed with `<ab_volume_pair_id>-a` or `<ab_volume_pair_id>-b`. For instance, an A/B volume pair comprised of two RAID arrays, `root-a` and `root-b`, would have an id `root`.

### Step 2: Run Trident to enable A/B update servicing

1. Run Trident to create the A/B volume pair on [clean install](../Reference/Glossary.md#clean-install), or adopt the A/B volume pair on [`offline-init`](../Explanation/Offline-Init.md). Trident will:

   - On clean install, create underlying A/B volume devices: disk partitions, RAID arrays, and/or encrypted volumes. On `offline-init`, adopt underlying A/B volume devices.
   - Link each pair of devices into a logical A/B volume pair.
   - Service volume A in each pair, so that it becomes active in the target OS.
   - If needed, mount volume A at the requested mount point after booting into the target OS.

1. Run an A/B update with Trident. During an A/B update, Trident will:

   - Service the inactive volume, so that it becomes active in the target OS.
   - If needed, mount the newly active volume at the mount point.

   **Important**: All A/B volume pairs will be updated in lockstep, meaning all pairs will have their A volumes be the active ones, or all pairs will have their B volumes be the active ones. This ensures system consistency across all A/B volumes.
