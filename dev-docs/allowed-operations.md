# Allowed Operations: stage vs finalize

A key feature of A/B upgrade with Trident is that **staging of new OS image**
**is decoupled from the reboot into the image**. In other words, the host does
not need to be rebooted immediately after the new OS image has been staged.

This is useful since the users often need to delay the reboot until a more
convenient point in time, when the workloads can be interrupted safely.
Moreover, after the new image has been staged, the user can repeatedly request
to have another OS image staged instead, before requesting to have the
servicing finalized. This is helpful in a scenario where an updated or a more
appropriate OS image becomes available.

This decoupled logic is implemented for **both clean install and A/B update.**
This is achieved by splitting the **--allowed-operations** option into `stage`
and `finalize`:

1. If the user includes `stage` into allowed operations, Trident will
stage the new OS image.
2. If `finalize` is included, Trident will finalize the deployment,
by setting UEFI variables, and trigger the host's reboot into the new OS image.

## Servicing Type and Servicing State

To track the progress of clean install or A/B upgrade and enable decoupling of
`stage` from `finalize`, Trident uses **TWO** objects:

1. **Servicing type**: `ServicingType` describes the type of changes required
based on Host Status and Host Configuration. This object has the following values:

   - `HotPatch`: Update that can be applied without pausing the workload.
   - `NormalUpdate`: Update that requires pausing the workload.
   - `UpdateAndReboot`: Update that requires rebooting the host.
   - `AbUpdate`:  Update that requires switching to a different root partition
      and rebooting.
   - `CleanInstall`: Clean install of the target OS image when the host is
      booted from the provisioning OS.
   - `NoActiveServicing`: No servicing is currently in progress.

2. **Servicing state**: `ServicingState` describes the current state of the
servicing done by Trident. The host will transition through a different
sequence of servicing states, depending on the servicing type that Trident is
executing. This object has the following values:

   - `NotProvisioned`: The host is running from the provisioning OS and has
      not yet been provisioned by Trident.
   - `CleanInstallStaged`: Clean install has been staged, i.e., the initial
      target OS images have been deployed onto block devices.
   - `AbUpdateStaged`: A/B update has been staged. The target OS images
      have been deployed onto block devices.
   - `CleanInstallFinalized`: Clean install has been finalized, i.e., UEFI
      variables have been set, so that firmware boots from the target OS image
      after reboot.
   - `AbUpdateFinalized`: A/B update has been finalized. For the next boot, the
      firmware will boot from the updated target OS image.
   - `Provisioned`: Servicing has been completed, and the host succesfully
      booted from the updated target OS image. Trident is ready to begin a new
      servicing.

## State Diagrams

The state diagrams below illustrate how `servicingState` of the host will
change in Host Status, depending on Host Configuration and the value(s)
provided in the `--allowed-operations` option:

### Clean Install State Diagram

::: mermaid
graph TD
    A[not-provisioned] --> |'stage' <br/>Valid HC received|B[not-provisioned]
    B --> |Staging failed|A
    B --> |Staging succeeded|C[clean-install-staged]
    C --> |'finalize'<br/>Finalizing succeeded|E[clean-install-finalized]
    C --> |'finalize'<br/>Finalizing failed|A
    C --> |'stage'<br/>Updated HC received|B
    E --> |Successfully booted from<br/>target OS image|G[provisioned]
    E --> |Failed to boot from<br/>target OS image|A

    %% Adjust node styles dynamically for content fitting
    style A white-space:normal,overflow-wrap:break-word,padding:10px
    style B white-space:normal,overflow-wrap:break-word,padding:10px
    style C white-space:normal,overflow-wrap:break-word,padding:10px
    style E white-space:normal,overflow-wrap:break-word,padding:10px
    style G white-space:normal,overflow-wrap:break-word,padding:10px

    %% Adjust edge text wrapping and size
    linkStyle 0 max-width:500px,white-space:normal,overflow-wrap:break-word
    linkStyle 1 max-width:300px,white-space:normal,overflow-wrap:break-word
    linkStyle 2 max-width:300px,white-space:normal,overflow-wrap:break-word
    linkStyle 3 max-width:300px,white-space:normal,overflow-wrap:break-word
    linkStyle 4 max-width:300px,white-space:normal,overflow-wrap:break-word
    linkStyle 5 max-width:300px,white-space:normal,overflow-wrap:break-word
    linkStyle 6 max-width:500px,white-space:normal,overflow-wrap:break-word
:::

### A/B Update State Diagram

::: mermaid
graph TD
    A[provisioned] --> |'stage'<br/>Valid HC received|B[provisioned]
    B --> |Staging failed|A
    B --> |Staging succeeded|C[ab-update-staged]
    C --> |'finalize'<br/>Finalizing succeeded|E[ab-update-finalized]
    C --> |'finalize'<br/>Finalizing failed|A
    C --> |'stage'<br/>Updated HC received|B
    E --> |Successfully booted from<br/>updated target OS|A
    E --> |Failed to boot from<br/>updated target OS<br/>and performed a rollback|A

    style A white-space:normal,overflow-wrap:break-word,padding:10px
    style B white-space:normal,overflow-wrap:break-word,padding:10px
    style C white-space:normal,overflow-wrap:break-word,padding:10px
    style E white-space:normal,overflow-wrap:break-word,padding:10px

    %% Adjust edge text wrapping and size
    linkStyle 0 max-width:500px,white-space:normal,overflow-wrap:break-word
    linkStyle 1 max-width:300px,white-space:normal,overflow-wrap:break-word
    linkStyle 2 max-width:300px,white-space:normal,overflow-wrap:break-word
    linkStyle 3 max-width:300px,white-space:normal,overflow-wrap:break-word
    linkStyle 4 max-width:300px,white-space:normal,overflow-wrap:break-word
    linkStyle 5 max-width:300px,white-space:normal,overflow-wrap:break-word
    linkStyle 6 max-width:500px,white-space:normal,overflow-wrap:break-word
:::
