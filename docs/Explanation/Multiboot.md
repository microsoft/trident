# Multiboot

Trident supports installing multiple operating systems on the same device. This
is accomplished by installing the new operating system to a separate set of
partitions from the existing operating system(s). For the existing operating
system(s), they must be marked as
[adopted](../How-To-Guides/Adopt-Existing-Partitions.md) so that Trident does
not attempt to modify them during install or servicing.

To enable this, use the
[`--multiboot`](../Reference/Trident-CLI.md#--multiboot-multiboot)
option for `trident install`.

Keep in mind that installing with multiboot carries some requirements that are
enforced by the [Trident safety check](./Clean-Install-Safety-Check.md).

When installing with multiboot, there will be only one ESP partition for the
machine. Trident will update the ESP and bootloader to include an entry for the
new operating system. The new operating system's EFI boot files will be put in
`/boot/efi/EFI/AZLXXX[A|B]` where `XXX` is the smallest available number (i.e.
`/boot/efi/EFI/AZL3A` and `/boot/efi/EFI/AZL3B` if `AZL1` and `AZL2` are
already taken).

When updating with multiboot, Trident will update the existing operating
system that is currently active. If you want to update a different operating
system, you must first boot into that operating system and then run
`trident update` from there.
