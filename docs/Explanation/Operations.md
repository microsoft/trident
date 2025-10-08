
# Operations

In addition to running `trident install` and `trident update` fully to
completion, Trident allows breaking these commands into separate `stage` and
`finalize` operations.

## Stage

Staging an update involves streaming partition images and applying the indicated
OS and bootloader configuration. It generally takes on the order of 1-5 minutes,
but because the modifications are occuring on the inactive A/B volume partitions,
the workload can continue running while the `stage` operation is in progress.

## Finalize

Finalizing an update configures the next boot and triggers a reboot.

* For `trident install`, this means that the UEFI BootOrder variable is
  configured to boot the newly installed OS on all subsequent reboots.

* For `trident update`, this means that the UEFI BootNext variable is
  configured to boot the newly installed OS on the next reboot. On the next
  boot, `trident commit` will validate the boot and update the UEFI BootOrder
  variable for all subsequent reboots.

To understand how to use these operations, see the
[Two-Step Installation and Update](../How-To-Guides/Two-Step-Installation-and-Update.md)
guide.
