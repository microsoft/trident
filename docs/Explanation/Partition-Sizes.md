# Partition Size Recommendations

This document provides recommendations for partition sizes in your Trident Host
Configuration. These sizes are used by Trident to manage the operating system
installation and update. The recommended sizes are based on typical use cases
and may need to be adjusted based on specific requirements.

For servicable partitions, like root, the sizes of the
[A/B volume pair](../Reference/Glossary.md#ab-volume-pair) must be the same.

## ESP

The EFI System Partition (ESP) is used for storing boot loaders and related
files.

For a single boot system, `512MB` is recommended.

For multiboot systems, each operating system should be accounted for. Azure
Linux should have at least `512MB`. For other operating systems, be aware of
their recommendations.

``` yaml
storage:
  disks:
    - id: os
      partitions:
        - id: esp
          type: esp
          size: 512M
```

## Boot

The boot partition is used for storing the kernel and initramfs files. The
recommended size for the boot partition is `256MB`.

``` yaml
storage:
  disks:
    - id: os
      partitions:
        - id: boot
          type: xbootldr
          size: 256M
```

## Root

The root partition size depends on the operating system being installed. The
minimum recommended size for root is `4GB`.

:::note
Using the minimal size does not leave much room for additional packages or
container images. Consider your use case and adjust the size accordingly.

``` yaml
storage:
  disks:
    - id: os
      partitions:
        - id: root
          size: 4G
```

## dm-verity Hash (root or usr)

When configuring a dm-verity system, you need to allocate space for the
dm-verity hash tree. The size of the hash tree depends on the size of the
partition being protected and the block size used for hashing.

[https://wiki.archlinux.org/title/Dm-verity#Partitioning](https://wiki.archlinux.org/title/Dm-verity#Partitioning)
suggests creating a hash partition that is 8-10% of the size of the partition
being protected.

``` yaml
storage:
  disks:
    - id: os
      partitions:
        - id: root-hash
          size: 256M
```

## Trident State

By default, Trident stores its state in `/var/lib/trident`. This includes logs
and other persistent state that Trident needs to operate. This path can be
customized, but because it is used to store state that persists across updates,
it must not be placed on an [A/B volume pair](../Reference/Glossary.md#ab-volume-pair).

Regardless of the location, the recommendation is to allocate at least
`256MB` for that partition.

``` yaml
storage:
  disks:
    - id: os
      partitions:
        - id: trident
          size: 256M
```

## Other Partitions

There are a lot of scenarios where you might want to define additional
partitions. People often create partitions to carve out dedicated spaces for
subtrees, e.g. /var.

These partitions should be sized according to your specific needs and use
cases.
