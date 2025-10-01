
# Adopt Existing Partitions

## Goals

By following this guide, you will be able to use Trident to install an operating system while [adopting or preserving some desired partitions](../Explanation/Partition-Adoption.md).

## Instructions

### Step 1: Determine Desired Partitions for Adoption

Find the desired adoption partitions on your host, either by:

* Partition labels, using a command like:

    ``` bash
    lsblk -o NAME,PARTLABEL
    ```

* UUIDs, using a command like:

    ``` bash
    lsblk -o NAME,UUID
    ```

### Step 2: Add `adoptedPartitions` Configuration

1. Inside the `storage` config, add a new `adoptedPartitions` section to the disk configuration for the disk containing the partitions you want to adopt. This section should include the partition IDs and their corresponding labels or UUIDs.

   For example:

   ```yaml
   storage:
     disks:
       - id: disk-with-partitions-to-adopt
         adoptedPartitions:
           - id: adopted-partition-by-label
             matchLabel: disklabel-part1
           - id: adopted-partition-by-uuid
             matchUuid: 12345678-abcd-1234-abcd-123456789abc
   ```

   With this information, Trident will ensure that these partitions are preserved in the new OS.

