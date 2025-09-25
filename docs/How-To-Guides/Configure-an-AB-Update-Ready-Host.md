
# Configure an A/B Update Ready Host

This guide explains how to configure the host to be ready for [A/B updates](../Reference/Glossary.md#ab-update), using the Host Configuration API.

## Goals

By following this guide, you will:

1. Declare A/B volume pairs on top of other devices using the Host Configuration.
1. Configure a host so that Trident can service it with A/B updates.

## Prerequisites

1. A host that has not yet been serviced by Trident.
1. A host configuration with the basic structure, including the [`storage`](../Reference/Host-Configuration/API-Reference/Storage.md) section.

## Instructions

### Step 0: Build a Target OS Suitable for A/B Updates

1. Build a target OS image that is suitable for A/B update servicing. As explained in the [glossary](../Reference/Glossary.md#ab-update), A/B update servicing requires **an A/B partition scheme**: two copies, or partitions, of the OS are kept on the system, and only one is active at a time. When an update is performed, the inactive copy is updated, and then the host is rebooted into the updated copy. Each volume that will be targeted with A/B updates must have two identical copies, A and B, present in the disk, forming a logical [A/B volume pair](../Reference/Glossary.md#ab-volume-pair). This means different things for the two main runtime flows:

- If you're following the [`offline-init`](../Explanation/Offline-Init.md) scenario, then the VM's disk layout must follow the A/B partition scheme, and the active partition A must have the initial OS image deployed onto it. [Image Customizer](https://microsoft.github.io/azure-linux-image-tools/imagecustomizer/README.html) can be used to build a VM image suitable for A/B updates, by listing device copies in the `disks.storage` Image Customizer configuration. This guide focuses on configuring an A/B update-ready host on **clean install**; to onboard a VM to Trident, follow [this tutorial](../Tutorials/Onboard-a-VM-to-Trident.md).
- If you're doing [a clean install](../Reference/Glossary.md#clean-install), then Trident will implement the A/B partition scheme for you. The target OS image, i.e. a COSI file, can be built by referencing this [tutorial](../Tutorials/Building-a-Deployable-Image.md). The OS image will target a single partition, A or B, at a time, so it must contain only a single set of volume copies.

### Step 1: Implement A/B Partition Scheme in `storage` Configuration

1. Add two copies of each volume to the `storage` configuration. To have Trident target a device with A/B updates, then the `storage` section must contain **exactly two** copies of that device that:

- Are disk partitions of any type, [RAID arrays](../Reference/Host-Configuration/API-Reference/Raid.md), or [encrypted volumes](../Reference/Host-Configuration/API-Reference/EncryptedVolume.md).
- Are of the **same** device type.
- Have the same size.

**Naming Convention**: In Trident, it is conventional to choose a short, descriptive string as the ID for an A/B volume pair. Then, to create the ids for the device copies inside the pair, the ID is suffixed with `<ab_volume_pair_id>-a` or `<ab_volume_pair_id>-b`. For instance, an A/B volume pair comprised of two RAID arrays, `root-a` and `root-b`, would have an ID `root`.

### Step 2: Add `abUpdate` configuration

1. Add a `storage.abUpdate` configuration to the host configuration. The [`abUpdate`](../Reference/Host-Configuration/API-Reference/AbUpdate.md) configuration carries information about the [A/B volume pairs](../Reference/Glossary.md#ab-volume-pair) that are used to perform A/B updates.

1. In the `abUpdate` configuration, add `volumePairs`. There can be multiple A/B volume pairs, as long as they are mounted at different mount points. This is a list of A/B volume pairs that will be targeted by A/B updates. Each A/B volume pair consists of two devices, A and B, that have the same type and size and are located in the same disk.

1. Add A/B volume pairs to [`volumePairs`](../Reference/Host-Configuration/API-Reference/AbVolumePair.md). Each A/B volume pair added to `volumePairs` must contain the following three **required** fields:

- `id` is a unique identifier for the A/B volume pair. This is a user-defined string that links the A/B volume pair to the results in the Host Status and to the [`filesystems`](../Reference/Host-Configuration/API-Reference/FileSystem.md) configuration. The identifier needs to be unique across devices of all types, not just A/B volume pairs.
- `volumeAId` is the ID of the device that will be used as the A volume.
- `volumeBId` is the ID of the device that will be used as the B volume.

  For example, the host configuration below declares one A/B volume pair with ID `root`. This A/B volume pair consists of two volumes, `root-a` and `root-b`, that are disk partitions. They have the same partition type `root` and are of the same size (8G). Because the `root` A/B volume pair needs to be mounted, the `filesystems` configuration lists `root` with the mount point at `/`.

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

### Step 3: Run Trident to Start A/B Update Servicing

1. Run Trident to create A/B volume pairs.

On a clean install, Trident will:

- Create underlying device copies: disk partitions, RAID arrays, and/or encrypted volumes.
- Link each pair of device copies into a logical A/B volume pair.
- Service volume A in each pair, so that it becomes active in the target OS.
- If needed, mount volume A at the requested mount point after booting into the target OS.

On `offline-init`, Trident will:

- Adopt underlying device copies.
- Link each pair of device copies into a logical A/B volume pair.

1. Run an A/B update with Trident. Trident will:

   - Update the OS image on the inactive partition, so that it becomes active after reboot.
   - If needed, mount the updated partitions at the mount point.

   **Important**: All A/B volume pairs will be updated in lockstep, meaning all pairs will have their A volumes be the active ones, or all pairs will have their B volumes be the active ones. This ensures system consistency across all A/B volumes.
