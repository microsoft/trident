
# UEFI Fallback

UEFI provides a mechanism for booting from an EFI file without a corresponding
boot variable existing in NVRAM. This is known as the UEFI fallback mode, and
it uses a specific file path (/EFI/BOOT) to locate the fallback bootloader.

Trident leverages this UEFI feature with
[UEFI fallback modes](../Reference/Host-Configuration/API-Reference/UefiFallbackMode.md)
by copying boot files into the UEFI fallback path during OS servicing. These
boot files determine what OS gets booted when the system is started in UEFI
fallback mode.

There are 3 UEFI fallback modes that determine which OS boot files are used
for the UEFI fallback path during OS servicing: `disabled`, `optimistic`, and
`conservative`.

Specifying `disabled` as the UEFI fallback mode means that Trident will not
populate the UEFI fallback boot files during OS servicing. This is useful
for multiboot scenarios or when the UEFI fallback is managed outside of
Trident. Note that if there are no UEFI fallback files present, the system may
not boot successfully if the UEFI variables are not found.

The remaining modes, `conservative` and `optimistic`, are designed to complement
Trident's [UEFI variable](./UEFI-Variables.md) management during OS servicing.

The default mode is `conservative` which aligns to how Trident manages the UEFI
boot variables during OS servicing.

* `trident install`
  * `finalize`: Trident updates the UEFI boot order **and UEFI fallback path**
    so that the target OS is booted.
  * `commit`: No changes to UEFI boot variables **or UEFI fallback path** are
    needed as the target OS is already configured to be booted.
* `trident update`
  * `finalize`: If the UEFI Fallback mode is `conservative`, the UEFI fallback path
    is updated to boot the **servicing OS**. This **aligns** to the UEFI variables.
  * `commit`: If the target OS boots successfully and the UEFI Fallback mode
    is `conservative`, the UEFI fallback path is updated to boot the target OS.

The following table summarizes how Trident manages UEFI fallback with `conservative`
during OS servicing:

| Trident Stage | `trident install` | `trident update` |
|---------------|-------------------|------------------|
| *stage* | UEFI fallback unchanged. | UEFI fallback unchanged. |
| *finalize* | UEFI fallback updated to boot target OS. | UEFI fallback updated to boot the servicing OS, meaning that any failures will cause the machine to boot into the servicing OS. |
| *commit* | No changes needed. | UEFI fallback updated to boot the target OS. |

`optimistic` is very similar to `conservative` but updates the UEFI fallback path
contents to the target OS earlier (during `finalize` instead of `commit`):

* `trident install`
  * `finalize`: Trident updates the UEFI boot order **and UEFI fallback path**
    so that the target OS is booted.
  * `commit`: No changes to UEFI boot variables **or UEFI fallback path** are
    needed as the target OS is already configured to be booted.
* `trident update`
  * `finalize`: If the UEFI Fallback mode is `optimistic`, the UEFI fallback
    path is updated to boot the **target OS**. This **differs** from the UEFI
    variables.
  * `commit`: If the target OS boots successfully and the UEFI Fallback mode
    is `optimistic`, no update to the UEFI fallback path is needed as it is
    already updated to boot the target OS.

The following table summarizes how Trident manages UEFI fallback with `optimistic`
during OS servicing:

| Trident Stage | `trident install` | `trident update` |
|---------------|-------------------|------------------|
| *stage* | UEFI fallback unchanged. | UEFI fallback unchanged. |
| *finalize* | UEFI fallback updated to boot target OS. | UEFI fallback updated to boot the target OS. |
| *commit* | No changes needed. | No changes needed. |

To summarize, the available UEFI fallback modes are:

* `conservative`: This is the default mode. `install` will configure the target OS
  as the UEFI fallback OS in `finalize`. `update` will configure the servicing
  OS as the UEFI fallback OS in `finalize` and the target OS in
  `commit` after verifying the boot.
* `optimistic`: `install` and `update` will configure the target OS as the UEFI
  fallback OS in `finalize`.
* `disabled`: No updates are made for the UEFI fallback path.

The UEFI fallback mode can be specified in the Host Configuration file under
the `os` section using the `uefiFallback` key. For example:

```yaml
os:
  uefiFallback: "optimistic"
```
