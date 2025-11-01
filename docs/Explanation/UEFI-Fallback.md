
# UEFI Fallback

UEFI provides a mechanism for booting from an EFI file without a corresponding
boot variable existing in NVRAM. This is known as the UEFI fallback mode, and
it uses a specific file path (\EFI\BOOT) to locate the fallback bootloader.

Trident leverages this UEFI feature with
[UEFI fallback modes](../Reference/Host-Configuration/API-Reference/UefiFallbackMode.md)
by copying boot files into the UEFI fallback path during OS servicing. These
boot files determine what OS gets booted when the system is started in UEFI
fallback mode.

There are 3 UEFI fallback modes that determine which OS boot files are used
for the UEFI fallback path during OS servicing: `none`, `rollforward`, and
`rollback`.

Specifying `none` as the UEFI fallback mode means that Trident will not
populate the UEFI fallback boot files during OS servicing. This is useful
for multiboot scenarios or when the UEFI fallback is managed outside of
Trident.

The default mode is `rollback` which aligns to how Trident manages the UEFI
boot variables during OS servicing. `rollforward` is very similar to `rollback`
but updates the UEFI fallback path contents to the target OS earlier (during
`finalize` instead of `commit`):

* `trident install`
  * `finalize`: Trident updates the UEFI boot order and UEFI fallback path so that
      the newly installed OS (the target OS) is booted.
  * `commit`: No changes to UEFI boot variables or fallback path are needed as the
      target OS is already configured to be booted.
* `trident update`
  * `finalize`: Trident updates the UEFI BootNext variable to boot the newly
      updated OS (the target OS) on the next reboot. The UEFI boot order is configured
      to **rollback** to the previous OS if the target OS fails to boot successfuly.
      If the UEFI Fallback mode is `rollback`, the UEFI fallback path is also updated
      to boot the previous OS (the servicing OS). If the UEFI Fallback mode is `rollforward`,
      the UEFI fallback path is updated to boot the newly serviced target OS.
  * `commit`: If the target OS boots successfully, Trident updates the UEFI boot
      order to boot the target OS on all subsequent reboots. If the UEFI Fallback mode
      is `rollback`, the UEFI fallback path is updated to boot the target OS. If the
      UEFI Fallback mode is `rollforward`, no update to the UEFI fallback path is needed
      as it is already updated to boot the newly serviced target OS.

To summarize, the available UEFI fallback modes are:

* `rollback`: This is the default mode. `install` will configure
  the target OS as the UEFI fallback OS in `finalize`.  `update` will configure
  the target OS as the UEFI fallback OS in `commit` after verifying the boot.
* `rollforward`: `install` and `update` will configure the target OS as the UEFI
  fallback OS in `finalize`.
* `none`: No updates are made for the UEFI fallback path. This is useful for
   multiboot or scenarios where the UEFI fallback is handled outside of Trident.

The UEFI fallback mode can be specified in the Host Configuration file under
the `os` section using the `uefiFallback` key. For example:

```yaml
os:
  uefiFallback: "rollforward"
```
