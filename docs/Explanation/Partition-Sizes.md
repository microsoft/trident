# Partition Size Recommendations

This document provides recommendations for partition sizes in your Trident Host Configuration.  These sizes are used by Trident to manage the operating system installation and update. The recommended sizes are based on typical use cases and may need to be adjusted based on specific requirements.

## ESP

The EFI System Partition (ESP) is used for storing boot loaders and related files. There are two recommended sizes for the ESP:

- `256MB ???` for single-OS GRUB systems
- `512MB ???` for single-OS UKI systems

For multiboot systems, `512MB * (number of operating systems) ???` is recommended.

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

The boot partition is used for storing the kernel and initramfs files. The recommended size for the boot partition is `200MB`.

``` yaml
storage:
  disks:
    - id: os
      partitions:
        - id: boot
          type: xbootldr
          size: 200M
```

## Root

The root partition size depends on the operating system being installed. The minimum recommended size for root is `4GB`.

``` yaml
storage:
  disks:
    - id: os
      partitions:
        - id: root
          size: 4G
```

## dm-verity Hash (root or usr)

When configuring a dm-verity system, you need to allocate space for the dm-verity hash tree. The size of the hash tree depends on the size of the partition being protected and the block size used for hashing.

[https://wiki.archlinux.org/title/Dm-verity#Partitioning](https://wiki.archlinux.org/title/Dm-verity#Partitioning) suggests creating a hash partition that is 8-10% of the size of the partition being protected.

``` yaml
storage:
  disks:
    - id: os
      partitions:
        - id: root-hash
          size: 256M
```

## Trident State

By default, Trident stores its state in `/var/lib/trident`. This includes logs, downloaded images, and other data that Trident needs to operate. This path can be customized, but regardless of the location, the recommendation is to allocate at least `1GB ???` for that partition.

``` yaml
storage:
  disks:
    - id: os
      partitions:
        - id: trident
          size: 1G
```
