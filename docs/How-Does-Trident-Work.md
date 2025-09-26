# How does Trident work?

Trident operates based on two primary concepts: [OS
installation](Reference/Glossary.md#install) and update, each corresponding to
dedicated CLI commands. Upon invocation, Trident receives a command along with a
Host Configuration that defines the desired state of the host. During initial
installation—or when performing offline initialization for a VM image—Trident
establishes its own datastore (implemented as a SQLite database) on the OS
filesystem, capturing the current state via the Host Status API.

For subsequent servicing operations, Trident compares the provided Host
Configuration against the most recent Host Status stored in its datastore. It
then queries its internal subsystems to determine the appropriate servicing type
required to reconcile the host with the desired configuration. Currently,
Trident supports the A/B update servicing model. In this model, Trident
pre-stages the OS image, applies additional OS configuration (including state
migration), updates the firmware boot configuration, and reboots into the newly
installed OS.

## Reliable deployment with safe rollback

In case issues arise during servicing, Trident configures the firmware to
automatically roll back to the previous OS version, ensuring system reliability.
Once the deployment is verified as successful, the user or orchestrator can
certify the deployment, prompting Trident to mark it accordingly and update the
firmware configuration to reflect the new state.

## Comprehensive state tracking

The Trident datastore maintains a comprehensive record of all servicing
operations, including the Host Configuration and Host Status for each operation.
This helps to diagnose any issues that may arise during servicing and provides a
clear audit trail of changes made to the host over time.

## Two-phase servicing workflow

Trident supports a two-phase servicing workflow:
[**stage**](Reference/Glossary.md#stage-operation) and
[**finalize**](Reference/Glossary.md#finalize-operation). During the **stage**
phase, updated OS image contents are copied onto the host without interrupting
running workloads, enabling preparation for servicing with minimal disruption.
Once staging is complete, workloads can be gracefully stopped, and the
**finalize** phase is initiated. In this phase, Trident updates the firmware
configuration to select the appropriate OS version for boot, and the host is
rebooted to complete the servicing operation. While both phases can be performed
right after another, they can also be separated by an arbitrary amount of time,
allowing for flexible scheduling and coordination with other maintenance tasks.
This two-phase approach minimizes workload downtime and ensures a smooth
transition to the updated OS.

To stage OS images, Trident utilizes the [COSI](Reference/COSI.md) file format
to enable efficient transfer and reliable deployment of the OS images. The file
format is designed so that Trident can stream individual filesystem images
directly from the specified source—such as a local file, HTTPS endpoint, or OCI
registry—into the target block device (partition, RAID array, LUKS volume,
etc.). During this process, Trident performs on-the-fly hashing and
decompression of filesystem blocks, ensuring rapid transfer without requiring
additional storage or memory for intermediate placement.

## Image validation and configuration

Leveraging metadata from the COSI file, Trident validates key aspects of the
source images, such as verifying that required dependencies are present, and
provides precise error reporting when issues are detected. Since COSI files can
be generated as an output format by Image Customizer, producing them is
straightforward. Trident employs COSI files for both OS installation and
updates, with the source location specified in the Host Configuration.

## Consistent desired state from first boot

Trident allows users to preconfigure all required changes during the staging
phase. As a result, after reboot, the OS boots directly into its desired
state—identical to any subsequent boot—eliminating the need for special handling
of the initial boot or post-boot configuration using tools like `cloud-init`
(although such tools can still be used if desired).

## State separation and controlled migration

Trident empowers users to separate the runtime state of the operating system
from the deployment image, as well as between the A and B OS instances. This is
achieved by enabling users to specify which volumes should be shared across A
and B instances and which should remain isolated. Trident promotes best
practices by encouraging users to minimize shared state between A and B, and to
explicitly migrate only the data that must persist during servicing operations.
Custom migration logic can be authored as needed, ensuring that state
preservation is both intentional and controlled. In the event of servicing
issues, Trident supports safe rollback to the previous OS state. Additionally,
where appropriate, Trident facilitates deduplication of data—such as container
image caches—between A and B instances, enabling workloads to resume rapidly
following a servicing reboot.

## Integration with Image Customizer

Trident seamlessly integrates with Image Customizer, enabling it to retrieve
image configuration details from both the COSI file and the separate Image
History file. This approach minimizes duplication of configuration data between
build and deployment phases, streamlining the overall workflow and ensuring
consistency.

## Reducing downtime

Trident is designed to minimize downtime during servicing operations. By staging
OS images in advance and allowing for a two-phase servicing workflow, Trident
ensures that workloads experience minimal disruption. Additionally, Trident's
efficient image transfer and deployment mechanisms further reduce the time
required for servicing operations.
