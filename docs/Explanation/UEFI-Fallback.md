
# UEFI Fallback

UEFI provides a mechanism for booting from an EFI file without a corresponding
boot variable existing in NVRAM. This is known as the UEFI fallback mode, and
it uses a specific file path (\EFI\BOOT) to locate the fallback bootloader.

Trident leverages this UEFI feature with
[UEFI fallback modes](../Reference/Host-Configuration/API-Reference/UefiFallbackMode.md)
by copying boot files into the UEFI fallback path during OS servicing. These
boot files determine what OS gets booted when the system is started in UEFI
fallback mode.

The available UEFI fallback modes are:

- `none`: This is the default mode and is supported for both `install` and
  `update`. In this mode, Trident will not make any changes to the fallback
  path. In this mode, if nothing outside of Trident has populated the fallback
  path, the system will not be able to boot.
- `rollforward`: This mode is supported for both `install` and `update`, where
  the boot files are copied from the target OS to the fallback path. For
  `install`, the newly installed OS will effectively be the fallback OS. For
  `update`, the newly updated OS will be the fallback OS. This ensures that if
  the system boots into fallback mode, it will boot into the most recently
  installed or updated OS.
- `rollback`: This mode is only supported for `update`, where the boot files
  from the servicing OS will be copied to the fallback path. This ensures that
  if the system boots into fallback mode, it will boot into the last
  successfully booted OS.

The UEFI fallback mode can be specified in the Host Configuration file under
the `os` section using the `uefiFallback` key. For example:

```yaml
os:
  uefiFallback: "rollforward"
```
