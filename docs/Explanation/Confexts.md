# Confexts

Systemd-Confexts (configuration extensions) are a mechanism for extending the
base OS filesystem with new configuration settings. Confexts extend the `/etc`
directory tree by mounting a read-only overlay over `/etc`. Please reference the
[systemd-confext man page](https://man.archlinux.org/man/systemd-confext.8.en)
for more information.

Trident supports servicing confexts as part of the [Clean
Install](../Reference/Glossary.md#clean-install) and [A/B
Update](../Reference/Glossary.md#ab-update) flows. Please reference the [confexts
API
documentation](../Reference/Host-Configuration/API-Reference/Os.md#confexts-optional)
for how to configure confexts in the Trident Host Configuration.

## Trident Configuration Notes

### Confext Path

If no `path` is specified for a confext in the Host Configuration, Trident will
default to placing the confext in `/var/lib/confexts/`. Trident currently
supports two other directories for placing confexts: `/usr/lib/confexts` and
`/usr/local/lib/confexts`. If A/B volumes are configured in the Host
Configuration, all confexts must be placed on an A/B volume. In other words,
Trident will return an error if `/var/lib/confexts/`, or any path specified in
the Host Configuration for a confext, is located on a shared volume.

### Confext Format

All confexts must be packaged as a [Discoverable Disk Image
(DDI)](https://uapi-group.org/specifications/specs/discoverable_disk_image/).
Trident expects to find exactly one valid extension-release file in the confext.
In addition, Trident requires that the confext contain the field `CONFEXT_ID` in
the extension-release file. This field is used to determine which confexts
require update during an A/B update flow. Each confext's `CONFEXT_ID` must be
unique among the IDs of all confexts listed in the Host Configuration.

### Read-Only Mount

Per systemd-confext documentation, confexts ["are strictly read-only by
default"](https://man.archlinux.org/man/systemd-confext.8.en). Mutable confexts
are not currently supported in Azure Linux 3.0 (systemd v255). It is important
to note that configuring confexts will result in `/etc` becoming read-only. This
can be problematic if other services that run on boot require writing to `/etc`.

### SELinux

Servicing of confexts is not currently compatible with SELinux. Therefore,
[SELinux](../Reference/Host-Configuration/API-Reference/Selinux.md) should be
configured to `disabled` in the Host Configuration.
