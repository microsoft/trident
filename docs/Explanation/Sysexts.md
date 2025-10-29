# Sysexts

Systemd-Sysexts (system extensions) are a mechanism for extending the base OS
filesystem with additional functionality and tooling. Sysexts extend the `/usr`
and `/opt` directory trees by mounting a read-only overlay over `/usr` and
`/opt`. Please reference the [systemd-sysext man
page](https://man.archlinux.org/man/systemd-sysext.8.en) for more information.

Trident supports servicing sysexts as part of the [Clean
Install](../Reference/Glossary.md#clean-install) and [A/B
Update](../Reference/Glossary.md#ab-update) flows. Please reference the [sysexts
API
documentation](../Reference/Host-Configuration/API-Reference/Os.md#sysexts-optional)
for how to configure sysexts in the Trident Host Configuration.

## Trident Configuration Notes

### Sysext Path

If no `path` is specified for a sysext in the Host Configuration, Trident will
default to placing the sysext in `/var/lib/extensions/`. Trident currently
supports two other directories for placing sysexts: `/etc/extensions/` and
`/.extra/sysexts`. If A/B volumes are configured in the Host Configuration, all
sysexts must be placed on an A/B volume. In other words, Trident will return an
error if `/var/lib/extensions/`, or any path specified in the Host Configuration
for a sysext, is located on a shared volume.

### Sysext Format

All sysexts must be packaged as a [Discoverable Disk Image
(DDI)](https://uapi-group.org/specifications/specs/discoverable_disk_image/).
Trident expects to find exactly one valid extension-release file in the sysext.
In addition, Trident requires that the sysext contain the field `SYSEXT_ID` in
the extension-release file. This field is used to determine which sysexts
require update during an A/B update flow.

### Read-Only Mount

Per systemd-sysext documentation, ["system extension images are strictly
read-only by default"](https://man.archlinux.org/man/systemd-sysext.8.en).
Mutable sysexts are not currently supported in Azure Linux 3.0 (systemd v255),
thus all sysexts will result in a read-only overlay over `/usr` and `/opt` (if
sysexts contain files in `/opt`).

### SELinux

Servicing of sysexts is not currently compatible with SELinux. Therefore,
[selinux](../Reference/Host-Configuration/API-Reference/Selinux.md) should be
configured to `disabled` in the Host Configuration.
