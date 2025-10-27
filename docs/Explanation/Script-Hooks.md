
# Script Hooks

Trident allows for users to run [custom
scripts](../Reference/Host-Configuration/API-Reference/Scripts.md) at three
different points during installation and update.

## Pre-Servicing Scripts

Pre-servicing scripts are run before Trident begins any operations. They are run
inside the [servicing OS](../Reference/Glossary.md#servicing-os). Pre-servicing
scripts are useful for:

- Saving the state of the servicing OS.
- Validating the state of the system before Trident starts servicing, i.e.
  checking for the presence of certain services.

## Post-Provision Scripts

Post-provision scripts are run inside the [servicing
OS](../Reference/Glossary.md#servicing-os). This script is run with root
filesystem of the [target OS](../Reference/Glossary.md#target-os) mounted at
`$TARGET_ROOT` and other partitions specified for the target OS mounted relative
to that. Post-provision scripts are useful for:

- Migrating configuration to the target OS.

## Post-Configure Scripts

Post-configure scripts are run inside the [target
OS](../Reference/Glossary.md#target-os). Post-configure scripts are useful for:

- Setting attributes or permissions for users.
- Installing packages required for expected workloads.
