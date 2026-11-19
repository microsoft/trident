# MSHV

This page covers the `efitreeloader-mshv-systemd-boot` COSI bootloader type
(`BootloaderType::EfitreeloaderMshvSystemdBoot` in Trident), including its boot
chain, file layout, and slot-owned servicing.

Azure Container Linux (ACL) images with MSHV support boot Linux as the root
partition of the Microsoft hypervisor. Servicing these images requires keeping
the MSHV boot components together with the OS version that uses them.
Trident's slot-owned MSHV mode extends [A/B updates](./AB-Update.md) to manage
those components alongside each OS slot.

## How This Boot Chain Differs

In a direct systemd-boot UKI chain, shim starts systemd-boot, which loads the
selected Unified Kernel Image (UKI). This bootloader type adds
[efitreeloader](https://github.com/microsoft/efitreeloader) and hypervisor-loading
components before systemd-boot:

```text
UEFI
  -> shim (bootx64.efi)
     -> efitreeloader (grubx64.efi)
        -> HvLoader.efi -> lxhvloader.dll -> hypervisor preparation
        -> systemd-boot (second.efi) -> selected UKI -> Linux with MSHV support
```

Despite its filename, `grubx64.efi` in this boot chain is efitreeloader, not GRUB.
HvLoader prepares the hypervisor-loading interface before systemd-boot loads
the Linux UKI.

A shared-path MSHV layout keeps MSHV boot components outside the slot
directories. Preserving those shared files during installation does not give
each A/B slot its own hypervisor version. Updating shared files could therefore
also change the MSHV boot components used by the retained OS.

Slot-owned servicing instead keeps MSHV boot components and their supporting
bootloaders together in each slot directory. The matching efitreeloader prefix
resolver locates its dependencies relative to the directory from which it runs.
The same boot files can consequently run from a managed slot or from the UEFI
fallback directory without referring to another slot's MSHV boot components.

Firmware boot-entry and UKI selection mechanisms remain unchanged; the
difference is which additional files belong to each slot.

## Boot Component Layout

The MSHV boot components are `HvLoader.efi`, `lxhvloader.dll`, and any
hypervisor or support binaries under `Windows`, including `Windows/System32`.
Shim, efitreeloader, and systemd-boot are supporting bootloaders, not
MSHV-specific components.
Each slot keeps both sets of files together.

The source COSI must contain the five top-level boot files shown below.
The optional `Windows` directory may contain image-specific MSHV
boot components; the examples below are not a fixed required list. Paths
are relative to the EFI System Partition (ESP), independent of its Linux
mount point.

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

COSI staging reuses ordinary shim and main-bootloader discovery and copying.
Efitreeloader additionally requires `second.efi`; MSHV additionally requires
`HvLoader.efi` and `lxhvloader.dll`. MSHV staging recursively selects additional
BOOT files and preserves their relative paths, including nested `Windows`
components, rather than imposing an exhaustive filename list. Root bootloader
and configuration files remain governed by ordinary discovery.
UKI handling is selected separately.
For the first OS installation, the managed layout is:

```text
EFI/
+-- BOOT/                   (fallback copy of a selected slot's boot files)
+-- AZLA/                   (slot A boot files)
+-- AZLB/                   (slot B boot files, populated when staged)
`-- Linux/                  (shared UKIs and their addon directories)
```

Each slot directory contains the same file hierarchy shown under BOOT.
UKIs and addons remain in `EFI/Linux`; they are not duplicated inside the
slot boot directories. See [Bootloader Configuration](./Bootloader-Configuration.md)
for UKI naming and managed boot-directory conventions.

## Enabling Slot-Owned Servicing

Use an x86-64 ACL UKI COSI containing the required BOOT files and built with
the matching efitreeloader prefix resolver. Its COSI metadata must set
`bootloader.type` to `efitreeloader-mshv-systemd-boot`, with a `systemdBoot`
section describing the standalone UKI entry.

Trident selects MSHV servicing from this bootloader type, without a Host
Configuration opt-in.

For first-update adoption, no source COSI describes the installed image.
Initialization seeds a missing AZLA from the complete BOOT tree using the
same structural checks for every boot type, without validating loader-specific
completeness. An existing AZLA is neither inspected nor refreshed.
Legacy shared files outside BOOT do not establish slot-owned mode.

## Updates, Fallback, and Rollback

Staging validates source boot files before clearing the target slot directory.
It copies the MSHV boot components and supporting bootloaders into the inactive
slot, then stages the OS's UKI and addons using the normal UKI servicing flow.
Staging relies on actual writes rather than predicting capacity; a write
failure returns an error and may leave partial inactive-slot files. The
retained slot's boot files are not replaced. Old target-slot UKIs and addons
may already have been removed to free space; failed staging does not restore them.

Fallback and bootstrap relocate complete installed boot trees without COSI
metadata or MSHV/efitreeloader filename selection. Files, nested directories
and empty directories are preserved. Fallback replacement retires
destination-only entries inside BOOT; bootstrap only seeds a missing AZLA.
Other slots and ESP directories are untouched.

Fallback replacement stages each top-level file or complete subtree as `.new`,
renames existing entries to `.old`, publishes the staged entries and removes
backups. Retries discard partial staging and reconcile backups. Replacement
across entries remains non-atomic. Bootstrap publishes its complete temporary
directory by rename. Neither path predicts free space: copy errors, including
space exhaustion, propagate before publication. Fallback leaves current entries
in place until every replacement entry has been staged; bootstrap leaves AZLA
absent until its temporary copy is complete.
Links, special files and source names ending in `.new` or `.old` are rejected.

[UEFI fallback modes](./UEFI-Fallback.md) apply to all BOOT files:

| Mode | Before the update reboot | After successful boot validation and health checks |
| --- | --- | --- |
| `conservative` | BOOT contains the retained slot's boot files. | Commit refreshes BOOT from the validated update target. |
| `optimistic` | BOOT already contains the update target's boot files. | No further fallback copy is needed. |
| `disabled` | BOOT is left unchanged. | No fallback copy is performed. |

Rollback selects the retained OS slot and its corresponding MSHV boot components.

## Current Limitations

The current implementation is limited to x86-64 ACL UKI images because the
boot-file definition hardcodes x64 filenames. TPM/PCR-bound encryption is
rejected.

These restrictions apply to file-based ESP deployment; streamed images do
not use this file-copy path. Offline initialization seeds AZLA when needed,
without MSHV-specific completeness checks.

### Changing the Boot Layout

Before A/B staging, validation compares the source COSI's bootloader type with the retained
slot's actual MSHV boot components. Converting between MSHV and ordinary boot
layouts is not supported. Manual rollback and fallback use the selected
slot's files, not a recorded configuration flag.

### Hypervisor Identity

**Hypervisor identity validation is not implemented.** Commit and rollback
do not query `/dev/mshv` or inspect ESP files to establish hypervisor identity.

Commit and rollback do not yet identify the running hypervisor or detect a
mixed hypervisor/OS boot. Root-device validation and health checks still
apply; passing them does not establish hypervisor/OS consistency or
production readiness.
