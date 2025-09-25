
# Clean Install Safety Check

When `trident install` is invoked, Trident ensures that it is running from either ramdisk or live media. If this is not the case, an error is returned immediately unless one of the following is true:

* [multiboot](./Multiboot.md) with [adopted partitions](../Reference/Host-Configuration/API-Reference/AdoptedPartition.md) is configured
* [multiboot](./Multiboot.md) is configured and the safety check override file (`/override-trident-safety-check`) is present
