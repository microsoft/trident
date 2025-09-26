
# Clean Install Safety Check

When [`trident install`](../Reference/Trident-CLI.md#install) is invoked, Trident ensures that it is running from either ramdisk or live media. This is an important safeguard and keeps Trident from overwriting the operating system that Trident is running in.

If Trident is not running in a ramdisk or live media, an error is returned immediately unless one of the following is true:

* [Multiboot](./Multiboot.md) with [adopted partitions](../Reference/Host-Configuration/API-Reference/AdoptedPartition.md) is configured.
* [Multiboot](./Multiboot.md) is configured and the safety check override file (`/override-trident-safety-check`) is present.
