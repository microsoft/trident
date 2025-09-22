
# Script Hooks

<!--
DELETE ME AFTER COMPLETING THE DOCUMENT!
---
Task: https://dev.azure.com/mariner-org/polar/_workitems/edit/13119
Title: Script Hooks
Type: Explanation
Objective:

Explanation of script hooks, when they run and why each is useful.
-->

Trident allows for users to run [custom
scripts](../Reference/Host-Configuration/API-Reference/Scripts.md) at three
different points during installation and update.

## Pre-Servicing Scripts

Pre-servicing scripts are run before Trident begins any operations. They are
useful for...

## Post-Provision Scripts

Post-provision scripts are run inside the management OS.

## Post-Configure Scripts

Post-configure scripts are run inside the [runtime
OS](../Reference/Glossary.md#runtime-os). 