
# Script Hooks

Trident allows for users to run [custom
scripts](../Reference/Host-Configuration/API-Reference/Scripts.md) at three
different points during installation and update.

## Pre-Servicing Scripts

Pre-servicing scripts are run before Trident begins any operations.
Pre-servicing scripts are useful for:

- Pre-save the state of the current OS

## Post-Provision Scripts

Post-provision scripts are run inside the current OS. In a clean install, the
script would run inside the [management
OS](../Reference/Glossary.md#management-os). In an A/B Update, the script would
run inside the current active volume. This script is run with root
filesystem of the target OS mounted at `$TARGET_ROOT` and other partitions
specified for the target OS mounted relative to that. Post-provision scripts are
useful for:

- Migrate configuration to the target OS

## Post-Configure Scripts

Post-configure scripts are run inside the [runtime
OS](../Reference/Glossary.md#runtime-os). Post-configure scripts are useful for:

- Setting attributes or permissions for users
- Installing packages
