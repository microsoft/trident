
# Bootloader Configuration

For installation and updating, Trident creates and modifies the bootloader of
the target OS. Both [`GRUB`](https://www.unixtutorial.org/reference/grub-bootloader/)
and [`systemd-boot`](https://www.freedesktop.org/software/systemd/man/latest/systemd-boot.html)
are supported.

The bootloader type is determined by the `COSI` file referenced in the
[Host Configuration](../Reference/Host-Configuration/API-Reference/HostConfiguration.md).

:::note
For [Unified Kernel Image (UKI)](https://uapi-group.org/specifications/specs/unified_kernel_image/),
Trident only supports using `systemd-boot`.
:::

To ensure Secure Boot compatibility, shim is configured as the primary
bootloader, which is booted by the firmware. The shim then boots the secondary
bootloader: either `systemd-boot` or `GRUB`. The Azure Linux keys, carried by
shim, are used to sign the secondary loaders and kernels.

## COSI Configuration

[Image Customizer](https://microsoft.github.io/azure-linux-image-tools/imagecustomizer/README.html)
supports creating COSI files that define either `GRUB` or `systemd-boot` as the
bootloader.

By default, Image Customizer creates `GRUB` based COSI files.

To create a systemd-boot COSI file, create a UKI-based
COSI file by ensuring that these settings are included in the
[COSI configuration file](https://microsoft.github.io/azure-linux-image-tools/imagecustomizer/api/configuration/uki.html#uki-type):

``` yaml
os:
  bootLoader:
    resetType: hard-reset
  uki:
    kernels: auto
previewFeatures:
- uki
```

:::note
The bootloader choice (and choice of UKI) has some implications on
[Encryption](../Reference/Host-Configuration/API-Reference/Encryption.md#pcrs-required).
:::

## Bootloader Servicing

:::note
Trident handles bootloader servicing for A/B updates without any required user
understanding or configuration. The following explanation is purely provided to
illustrate how this process works.
:::

In order to support A/B updates from a single ESP partition, Trident needs to
manage the bootloader files on the ESP partition in a particular way. The
bootloader is first installed as part of the COSI deployment.

As the target OS ESP partition is not an A/B volume pair, Trident will manage
this single partition so that it boots the active OS from the correct A/B OS
partitions.

At a high level, Trident will use the ESP partition from the COSI file as the basis
for the target OS's ESP partition. However, the layout and some file names
will differ slightly between the COSI ESP image and the target OS ESP. These
changes help Trident track multiple installs and A/B updates while ensuring
that the bootloader starts the correct OS.

To handle A/B updates, Trident will assume two bootloader paths, an `A` and a
`B` path.

For example, for a simple `trident install`, the target OS bootloader paths will
be:

* `/boot/efi/EFI/AZLA` - the target OS for the initial install
* `/boot/efi/EFI/AZLB` - the target OS for a future update

Within the bootloader paths, Trident will copy EFI files (like `boot<ARCH>.efi`
and `grub<ARCH>.efi`) and the `grub.cfg` from the COSI ESP image.

### systemd-boot Bootloader

Trident will copy and rename the UKI EFI file from the COSI ESP image, where
it is versioned with the kernel version (for example
`/boot/efi/EFI/Linux/vmlinuz-6.6.96.2-2.azl3.efi`), to
[`/boot/efi/EFI/Linux`](https://uapi-group.org/specifications/specs/boot_loader_specification/#locating-boot-entries)
on the target OS. Trident will rename the UKI EFI file to ensure that the
correct file is loaded at boot, as `systemd-boot` will sort the UKI files by
their names and load the most recent one.

To ensure that the most recent UKI, representing the correct partition, is
loaded, Trident follows this naming convention:

`vmlinuz-[SERVICING_INDEX]-azl[ACTIVE_PARTITION][OS_INDEX].efi`

Where:

* `[SERVICING_INDEX]` is incremented for each servicing operation (install or
  update). The index is started at 100 to avoid conflicts with the standard
  kernel-version-based naming.
* `[ACTIVE_PARTITION]` is either `a` or `b` reflecting the active boot
  partition
* `[OS_INDEX]` is the 0-based index of the operating system being booted

For example, the first install of a single OS would create
`/boot/efi/EFI/Linux/vmlinuz-100-azla0.efi`. A subsequent update would create
`/boot/efi/EFI/Linux/vmlinuz-101-azlb0.efi`. The next update would create
`/boot/efi/EFI/Linux/vmlinuz-102-azla0.efi`, and so on.
