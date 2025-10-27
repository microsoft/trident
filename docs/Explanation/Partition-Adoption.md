
# Partition Adoption

When Trident installs or services an operating system, there are times when
existing partitions need to be preserved in the target OS. These partitions
can be adopted by modifying the Trident Host Configuration to include
[identifying details for these partitions](../Reference/Host-Configuration/API-Reference/AdoptedPartition.md).
These identifying details can be either (but not both) a label or a UUID.

> Note: Any other pre-existing partitions, which are not declared in the Host
> Configuration to be adopted, will be over-written by Trident during
> servicing.

For example, to adopt 2 partitions from a single disk, one with a matching
label (`disklabel-part1`) and one with matching UUID
(`12345678-abcd-1234-abcd-123456789abc`), add this to your Host
Configuration:

``` yaml
storage:
  disks:
    - id: disk-with-partitions-to-adopt
      adoptedPartitions:
        - id: adopted-partition-by-label
          matchLabel: disklabel-part1
        - id: adopted-partition-by-uuid
          matchUuid: 12345678-abcd-1234-abcd-123456789abc
```

> Note: Partitions must adhere to some
> [partition type requirements](../Reference/Host-Configuration/Storage-Rules.md)
> to be adopted.

Please refer to [Adopt Existing Partitions](../How-To-Guides/Adopt-Existing-Partitions.md)
for more details on how to adopt partitions.
