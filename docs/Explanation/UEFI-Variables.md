# UEFI Variables

Trident uses UEFI variables to manage a machine's boot process. There are
several UEFI variables that Trident interacts with, including:

* `Boot entries (Boot####)`: These variables represent individual boot entries,
  where `####` is a hexadecimal number (e.g., `Boot0001`, `Boot0002`, etc.).
  Each boot entry contains information about a specific boot option, such as
  the path to the bootloader and any associated parameters.
* `BootOrder`: This variable defines the order in which UEFI boot entries are
  attempted during system startup.
* `BootNext`: This variable specifies the next boot entry to be used on the
  next reboot.

Trident manages these variables during OS servicing to ensure that the system
is always in a bootable state and to ensure that the desired OS is booted.

* `trident install`
  * `finalize`: Trident updates the `BootOrder` so that the target OS (the
    newly installed OS) is booted.
  * `commit`: No changes to UEFI variables are needed as the target OS is
    already configured to be booted.
* `trident update`
  * `finalize`: Trident updates the `BootNext` variable to boot the target OS
    (the newly updated OS) on the next boot. The `BootOrder` is still
    configured to boot the servicing OS (the previous OS). This enables the
    machine to **rollback** to the previous OS if the target OS fails to boot
    successfuly.
  * `commit`: If the target OS boots successfully, Trident updates the
    `BootOrder` to boot the target OS on all subsequent boots.

The following table summarizes how Trident manages UEFI variables during OS
servicing:

| Trident Stage | `trident install` | `trident update` |
|---------------|-------------------|------------------|
| *stage* | UEFI variables are unchanged. | UEFI variables are unchanged. |
| *finalize* | `BootOrder` updated to boot target OS. | `BootNext` updated to boot target OS, ensure target OS is last in `BootOrder`. This means that the next boot will boot the Target OS, but any failures will cause the machine to boot into the servicing OS. |
| *commit* | UEFI variables are unchanged. | The target OS `BootEntry` is moved to first in `BootOrder`. |
