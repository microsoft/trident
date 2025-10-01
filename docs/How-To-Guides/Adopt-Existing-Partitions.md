
# Adopt Existing Partitions

## Goals

By following this guide, you will ber able to use Trident to install an operating system while [adopting](../Explanation/Partition-Adoption.md) (or persisting) some desired partitions.

## Instructions

### Step 1: Determine Desired Partitons for Adoption

Find the desired adoption partitions on your host, either by:

* Partition labels, using a command like:

    ``` bash
    ls -l /dev/disk/by-id
    ```

* UUIDs, using a command like:

    ``` bash
    ls -l /dev/disk/by-uuid
    ```

### Step 2: Add `adoptedPartitions` Configuration

1. Inside the `storage` config, add a new `adoptedPartitions` section to the disk configuration for the disk containing the partitions you want to adopt. This section should include the partition IDs and their corresponding labels or UUIDs.

   For example:

   ```yaml
   storage:
     disks:
       - id: disk-with-partitions-to-adopt
         adopted_partitions:
           - id: adopted-partition-by-label
             matchLabel: disklabel-part1
           - id: adopted-partition-by-uuid
             matchUuid: 12345678-abcd-1234-abcd-123456789abc
   ```

   With this information, Trident will ensure that these partitions are preserved in the new OS.

