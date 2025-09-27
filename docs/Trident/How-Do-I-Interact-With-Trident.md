---
sidebar_position: 3
---

# How do I interact with Trident?

Trident is architected for seamless integration into larger solutions. Its
primary responsibility is single-host servicing, while orchestration logic—such
as scheduling and input selection—is delegated to external systems. This
approach ensures that product-specific orchestrators, which possess deeper
insight into deployment requirements and timing across a fleet, can efficiently
manage operations. Trident simplifies deployment by leveraging a declarative
Host Configuration, enabling consistent and reliable servicing without imposing
unnecessary complexity.

Trident provides a robust command-line interface (CLI) for managing OS
installation and servicing operations. The CLI supports the following commands:

- [`install`](How-To-Guides/Perform-a-Clean-Install.md): Initiates the initial
  installation of the operating system.
- [`offline-initialize`](Tutorials/Onboard-a-VM-to-Trident.md): Prepares the
  Trident datastore for a VM image, enabling future in-place servicing. This is
  typically performed during VM image creation.
- [`update`](Tutorials/Performing-an-ABUpdate.md): Executes an OS update in
  accordance with the supplied Host Configuration.
- `commit`: Certifies the current OS deployment as successful.
- [`rebuild-raid`](How-To-Guides/Rebuild-RAID-Array.md): Reconstructs a degraded
  software RAID array following physical drive replacement.
- `get`: Retrieves the most recent Host Configuration, Host Status, or error
  details. This is particularly useful for non-interactive scenarios.
- [`validate`](How-To-Guides/Host-Configuration-Validation.md): Validates that a
  provided Host Configuration is syntactically correct and self consistent. Can
  be run on any system including a development machine, and thus does not
  consider the current system state.

Please consult [CLI reference](Reference/Trident-CLI.md) for detailed
information on each command and its usage.

Trident is designed for both interactive use by administrators and
non-interactive integration with orchestration systems. In automated
environments, orchestrators can utilize the `get` command to monitor operation
status and determine appropriate next steps based on structured feedback from
Trident.

Trident logs its operations to standard output, which can be redirected to a
file or monitored in real-time. The logging verbosity can be adjusted using the
`--verbosity` flag, allowing users to tailor the level of detail to their needs.
Trident also creates a [detailed log
file](How-To-Guides/View-Trident's-Background-Log.md) at
`/var/log/trident-full.log` for post-operation review and troubleshooting.
