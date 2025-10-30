
# UEFI Fallback

Trident supports [UEFI fallback modes](../Reference/Host-Configuration/API-Reference/UefiFallbackMode.md) to handle situations where the UEFI firmware becomes corrupted or misconfigured, preventing the system from booting correctly. The configured fallback modes determine how the system should respond in such scenarios and are applied during servicing.

The available UEFI fallback modes are:

- `none`: This is the default mode and is supported for both `install` and `update`. In this mode, Trident will not make any changes to the fallback path.
- `rollforward`: This mode is supported for both `install` and `update`, where the boot files are copied from the target OS to the fallback path. For `install`, the newly installed OS will effectively be the fallback OS. For `update`, the newly updated OS will be the fallback OS.
- `rollback`: This mode is only supported for `update`, where the boot files from the servicing OS will be copied to the fallback path.

The UEFI fallback mode can be specified in the Host Configuration file under the `os` section using the `uefiFallback` key. For example:

```yaml
os:
  uefiFallback: "rollforward"
```
