
# SELinux Configuration

SELinux is an access control system on Linux, officially supported by the Azure
Linux distribution. SELinux is a Mandatory Access Control (MAC) system, meaning
that the policy is set by a security administrator and cannot be changed by
users. SELinux works in tandem with other security mechanisms; it cannot grant
access that is denied by other mechanisms. SELinux is useful for achieving
security goals such as:

- Least privilege
- Integrity
- Isolation
- Confidentiality
- Role separation

The primary mechanism in SELinux is Type Enforcement (TE). In this mechanism,
every process and object in the system has a type​. TE rules allow access
between types​. This can be thought of like an access matrix. All access that is
not explicitly allowed is denied​. TE rules comprise over 99% of the SELinux
policy​. In order for processes and objects to have the correct type, the
`setfiles` command relabels files with the appropriate type.

An example rule from the [Trident SELinux
policy](../../selinux-policy-trident/trident.te):

```te
allow trident_t tmpfs_t:filesystem { getattr mount unmount };
```

This rules allow processes with the `trident_t` type, i.e. Trident, to access
filesystems with type `tmpfs_t` and perform the operations `getattr`, `mount`,
and `unmount`.

## Trident SELinux Domain

When run directly on the host with SELinux enabled, Trident will run in the
domain `trident_t`. The Trident SELinux policy is defined in
`selinux-policy-trident/` directory. On the other hand, the Trident container
image runs in privileged mode and thus runs in the `spc_t` domain, i.e "Super
Privileged Container". As a result, the policies related to `trident_t` do not
apply to the Trident container image.

As part of its operations, Trident will run
[`setfiles`](https://man7.org/linux/man-pages/man8/setfiles.8.html) on the new
OS. This operation relabels all of the files in the new OS (what will become the
[runtime OS](../Reference/Glossary.md)) according to the labels specified at
`/etc/selinux/targeted/contexts/files/file_contexts`.

## Configuring SELinux for the Runtime OS

Trident allows users to configure the state of SELinux in the [runtime
OS](../Reference/Glossary.md#runtime-os) using the [`os.selinux`
API](../Reference/Host-Configuration/API-Reference/Selinux.md). SELinux can be
configured to be in the following modes:

- `enforcing`: All SELinux policies are enforced and any denials from the
SELinux security module will result in processes being terminated. Denials are
also logged at `/var/log/audit/audit.log`.
- `permissive`: SELinux policies are not enforced. All denials are logged at
`/var/log/audit/audit.log`.
- `disabled`: SELinux policies are neither enforced nor logged.

Note that in order for the SELinux configuration in the Host Configuration to
take effect, SELinux must be present in the [runtime OS
OS](../Reference/Glossary.md#runtime-os)'s image.

| Host Configuration \ Provisioning OS | NOT PRESENT | DISABLED  | PERMISSIVE | ENFORCING |
|--------------------------------------|-------------|-----------|------------|-----------|
| NOT PRESENT                          | NOT PRESENT | DISABLED  | PERMISSIVE | ENFORCING |
| DISABLED                             | NOT PRESENT | DISABLED  | DISABLED   | DISABLED  |
| PERMISSIVE                           | Error       | PERMISSIVE| PERMISSIVE | PERMISSIVE|
| ENFORCING                            | Error       | ENFORCING | ENFORCING  | ENFORCING |

Trident will determine whether or not SELinux is available on the [runtime
OS](../Reference/Glossary.md#runtime-os) by checking for `/etc/selinux/config`.

## Debugging SELinux Denials

SELinux emits messages using the Linux kernel audit subsystem. If the system has
an auditd service running, the audit logs are available in
`/var/log/audit/audit.log`. To search for SELinux denial messages, use:

```bash
ausearch -m AVC
```

If auditd is not running, the messages will be in the system log, typically in
`/var/log/messages`. On systemd systems, search for SELinux denial messages
using journalctl, filtering on the SELinux denial audit type:

```bash
journalctl _AUDIT_TYPE_NAME=AVC
```
