
# SELinux Configuration

<!--
DELETE ME AFTER COMPLETING THE DOCUMENT!
---
Task: https://dev.azure.com/mariner-org/polar/_workitems/edit/13168
Title: SELinux Configuration
Type: Explanation
Objective:

Explanation of the SELinux configuration done by Trident.
-->

SELinux is a security module in the Linux kernel that locks down the operations
that all processes and users are allowed to perform.

## Trident SELinux Domain

When run directly on the host with SELinux enabled, Trident will run in the
domain `trident_t`. The Trident SELinux policy is defined in
`selinux-policy-trident/` directory. On the other hand, the Trident container
image runs in privileged mode and thus runs in the `spc_t` domain, i.e "Super
Privileged Container". As a result, the policies related to `trident_t` do not
apply to the Trident container image.

As part of its operations, Trident will run
(`setfiles`)[https://man7.org/linux/man-pages/man8/setfiles.8.html] on the new
OS. This operation relabels all of the files in the new OS (what will become the
(runtime OS)[../Reference/Glossary.md]) according to the labels specified at
`/etc/selinux/targeted/contexts/files/file_contexts`.

## Configuring Trident for the Runtime OS

Trident allows users to configure the state of SELinux in the (runtime
OS)[../Reference/Glossary.md] using the (`os.selinux`
API)[../Reference/Host-Configuration/API-Reference/Selinux.md]. SELinux can be
configured to be in `enforcing`, `permissive`, or `disabled` mode. Note that in
order for the SELinux configuration in the Host Configuration to take effect,
SELinux must be present on the underlying (provisioning
OS)[../Reference/Glossary.md].

| Host Configuration \ Provisioning OS       | NOT PRESENT | DISABLED  | PERMISSIVE | ENFORCING |
|---------------|-------------|-----------|------------|-----------|
| NOT PRESENT   | NOT PRESENT | DISABLED  | PERMISSIVE | ENFORCING |
| DISABLED      | NOT PRESENT | DISABLED  | DISABLED   | DISABLED  |
| PERMISSIVE    | Error       | PERMISSIVE| PERMISSIVE | PERMISSIVE|
| ENFORCING     | Error       | ENFORCING | ENFORCING  | ENFORCING |
