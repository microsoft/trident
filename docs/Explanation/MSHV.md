# MSHV

Azure Container Linux (ACL) images with MSHV support boot Linux as the root
partition of the Microsoft hypervisor. Servicing these images requires keeping
the hypervisor's boot payload together with the OS version that uses it.
Trident's slot-owned MSHV mode extends [A/B updates](./AB-Update.md) to manage
that payload alongside each OS slot.

## How MSHV Boot Differs

In an ordinary UKI-based image, shim starts systemd-boot, which loads the
selected Unified Kernel Image (UKI). An MSHV image adds
[efitreeloader](https://github.com/microsoft/efitreeloader) and hypervisor-loading
components before systemd-boot:

```text
UEFI
  -> shim (bootx64.efi)
     -> efitreeloader (grubx64.efi)
        -> HvLoader.efi -> lxhvloader.dll -> hypervisor payload preparation
        -> systemd-boot (second.efi) -> selected UKI -> Linux with MSHV support
```

Despite its filename, `grubx64.efi` in this bundle is efitreeloader, not GRUB.
HvLoader prepares the hypervisor-loading interface before systemd-boot loads
the Linux UKI.

A shared-path MSHV layout keeps hypervisor components outside the slot
directories. Preserving those shared files during installation does not give
each A/B slot its own hypervisor version. Updating shared files could therefore
also change the payload used by the retained OS.

Slot-owned servicing instead keeps the complete boot payload in each slot
directory. The matching efitreeloader prefix resolver locates its dependencies
relative to the directory from which it runs. The same bundle can consequently
run from a managed slot or from the UEFI fallback directory without referring
to another slot's hypervisor files.

Firmware boot-entry and UKI selection mechanisms remain unchanged; the
difference is which additional files belong to each slot.

## Payload Layout

The source COSI must contain this complete bundle. Paths below are relative
to the EFI System Partition (ESP), independent of its Linux mount point.

```text
EFI/
`-- BOOT/
    +-- bootx64.efi          (shim)
    +-- grubx64.efi          (efitreeloader)
    +-- second.efi           (systemd-boot)
    +-- HvLoader.efi
    +-- lxhvloader.dll
    `-- Windows/
        `-- System32/
            +-- hvix64.exe   (Intel hypervisor)
            +-- hvax64.exe   (AMD hypervisor)
            `-- kdstub.dll
```

Trident copies all eight files, preserving the nested directories. For the
first OS installation, the managed layout is:

```text
EFI/
+-- BOOT/                   (fallback copy of a selected slot's complete bundle)
+-- AZLA/                   (complete slot A bundle)
+-- AZLB/                   (complete slot B bundle, populated when staged)
`-- Linux/                  (shared UKIs and their addon directories)
```

Each slot directory contains the same file hierarchy shown under BOOT.
UKIs and addons remain in `EFI/Linux`; they are not duplicated inside the
slot bundles. See [Bootloader Configuration](./Bootloader-Configuration.md)
for UKI naming and managed boot-directory conventions.

## Enabling Slot-Owned Servicing

Use an x86-64 ACL UKI COSI containing the complete BOOT bundle and built with
the matching efitreeloader prefix resolver. Include this flag in the
[Host Configuration](../Reference/Host-Configuration/API-Reference/HostConfiguration.md):

```yaml
internalParams:
  mshvSlotEspPayload: true
```

Enable the flag during clean installation or first-update ACL adoption, and
keep it enabled for subsequent updates.

For first-update adoption, Trident discovers the mode from the installed
image's BOOT bundle and requires the request to match. It seeds the complete
AZLA bundle before persisting the discovered HostStatus. Incomplete MSHV
bundles fail discovery; legacy shared files outside BOOT do not establish
slot-owned mode.

## Updates, Fallback, and Rollback

Staging validates the source bundle and estimates the ESP space required
before clearing the target slot directory. It then copies the complete
bundle into the inactive slot and stages the OS's UKI and addons using the
normal UKI servicing flow. The retained slot's bundle is not replaced.

[UEFI fallback modes](./UEFI-Fallback.md) apply to the complete BOOT bundle:

| Mode | Before the update reboot | After successful boot validation and health checks |
| --- | --- | --- |
| `conservative` | BOOT contains the retained slot's bundle. | Commit refreshes BOOT from the validated update target. |
| `optimistic` | BOOT already contains the update target's bundle. | No further fallback copy is needed. |
| `disabled` | Trident does not populate BOOT. | No fallback copy is performed. |

Rollback selects the retained OS slot and its corresponding payload.
A separate hypervisor-validation failure state supports automatic recovery
without classifying an identity failure as a user health-check failure.

## Current Limitations

The current implementation is limited to x86-64 ACL UKI images because the
payload definition hardcodes x64 filenames. TPM/PCR-bound encryption and
combining this mode with `preserveMshvEspPayload` are rejected.

Offline initialization of MSHV-enabled ACL images is not currently supported.

### Changing the Recorded Mode

Here, "mode" means the `internalParams.mshvSlotEspPayload` flag, not whether
the kernel supports MSHV. An installation is already managed once Trident has
persisted its HostStatus. Subsequent update requests and historical
configurations selected for manual rollback must retain the recorded flag:

* If recorded as enabled, keep `mshvSlotEspPayload: true`. Setting it to false
  or omitting it from an update request is rejected.
* If recorded as disabled, setting `mshvSlotEspPayload: true` is rejected,
  even if the new COSI contains an MSHV bundle.

Trident does not convert an existing shared/non-MSHV payload layout into
slot-owned management, or convert a slot-owned installation back out of it.
Normal A/B updates and rollbacks that retain the same flag remain supported.
The initial mode is established during clean installation or discovered
during first-update ACL adoption, before an installation is already managed.

### Hypervisor Identity

**Hypervisor identity validation currently assumes success.** Commit and
rollback do not yet identify the running hypervisor
or detect a mixed hypervisor/OS boot. Root-device validation and health checks
still apply; passing them does not establish hypervisor/OS consistency or
production readiness.
