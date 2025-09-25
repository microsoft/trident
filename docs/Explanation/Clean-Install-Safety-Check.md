
# Clean Install Safety Check

When `trident install` is invoked, Trident ensures that it is running from either ramdisk or live media. If this is not the case, an error is returned immediately unless one of the following is true:

* multiboot with adopted partitions is configured
* multiboot is configured and the safety check override file (`/override-trident-safety-check`) is present
