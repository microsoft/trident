
# SELinux Configuration

SELinux is a security module in the Linux kernel that locks down the operations
that all processes and users are allowed to perform. SELinux operates as a
Mandatory Access Control (MAC) system, meaning that the kernel enforces the
access control, defined by policy rules that are currently enabled. Users and
processes do not have permission to change the security rules. SELinux is useful
for ensuring that processes and users do not perform actions they are not
explicitly allowed to perform, and wards against malware.

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
configured to be in `enforcing`, `permissive`, or `disabled` mode. In
`enforcing` mode, all SELinux policies are enforcing and any denials from the
SELinux security module will result in processes being terminated. In
`permissive` mode, SELinux policies are not enforced and any denials are instead
logged at `/var/log/audit/audit.log`. Lastly, in `disabled` mode, SELinux
policies are neither enforced nor logged.

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
