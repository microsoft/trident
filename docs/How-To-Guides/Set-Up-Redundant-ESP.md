
# Set Up Redundant ESP

This guide shows you how to configure an operating system with ESP
redundancy. Follow the steps below to create a Trident Host
Configuration that configures ESP on a RAID volume.

## Goals

By following this guide, you will understand how to configure an
ESP on a RAID array

## Instructions

The required configurations should all be made in the Trident Host
Configuration. A detailed explanation of creating RAID arrays can be
found in the [Create a RAID Array guide](./Create-a-RAID-Array.md).
For this guide, we break RAID creation into 2 parts:

1. To benefit from redundancy, create a RAID array that utilizes multiple
   disks by defining 2 partitions for `raid1` (`esp-1` and `esp-2`) like this:

    ``` yaml
    storage:
      disks:
        - id: disk1
          device: /dev/disk/by-path/pci-0000:00:1f.2-ata-2
          partitionTableType: gpt
          partitions:
            - id: esp-1
              type: esp
              size: 1G
        - id: disk2
          device: /dev/disk/by-path/pci-0000:00:1f.2-ata-3
          partitionTableType: gpt
          partitions:
            - id: esp-2
              type: esp
              size: 1G
    ```

2. Create a `raid` section in `storage` section of your Trident Host
   Configuration that combines these partitions into a RAID array:

    ``` yaml
    storage:
      raid:
        software:
          - id: esp
            name: esp
            level: raid1
            devices:
              - esp-1
              - esp-2
    ```

Having created the RAID array, it can then be referenced to host the ESP
filesystem:

``` yaml
  filesystems:
    - deviceId: esp
      mountPoint:
        path: /boot/efi
        options: umask=0077
```
