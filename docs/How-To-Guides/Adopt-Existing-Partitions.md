
# Adopt Existing Partitions

## Goals

By following this how-to guide, you will learn how
to [adopt existing partitions](../Explanation/Partition-Adoption.md)
with Trident, by using Host Configuration.

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

Inside the `storage` section of your Trident Host Configuration,
add a new [adoptedPartitions](../Reference/Host-Configuration/API-Reference/AdoptedPartition.md)
section to the `disk` section containing the partitions you want
to adopt. This section should include the partition IDs and their
corresponding uniquely identifying labels _or_ UUIDs.

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

To adopt the filesystem on the above partition:

``` yaml
  filesystems:
    - deviceId: disk-with-partitions-to-adopt
      source: adopted
      mountPoint: /adopted-filesystem
```

With this information, Trident will ensure that these partitions are
preserved in the target OS.

