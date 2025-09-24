---
sidebar_position: 1
---

# Trident - Azure Linux Servicing Agent

**Trident** is a declarative, security-first OS lifecycle agent designed
primarily for [Azure
Linux](https://github.com/microsoft/azurelinux/?tab=readme-ov-file#azure-linux).
It supports installation and provisioning of bare-metal nodes, as well as
A/B-style atomic updates and runtime configuration for both bare-metal and
virtual machines.

- [Trident - Azure Linux Servicing Agent](#trident---azure-linux-servicing-agent)
  - [Motivation](#motivation)
  - [What is Trident?](#what-is-trident)
  - [How does Trident work?](#how-does-trident-work)
  - [How do I interact with Trident?](#how-do-i-interact-with-trident)
  - [What can Trident do?](#what-can-trident-do)
  - [See a prerecorded demo of Trident in action](#see-a-prerecorded-demo-of-trident-in-action)
  - [How can I get started?](#how-can-i-get-started)
    - [Found an issue or missing a feature?](#found-an-issue-or-missing-a-feature)
    - [Try out Trident](#try-out-trident)
      - [Do you want to deploy a bare metal host?](#do-you-want-to-deploy-a-bare-metal-host)
      - [Do you want to update a bare metal host or a virtual machine?](#do-you-want-to-update-a-bare-metal-host-or-a-virtual-machine)
      - [Do you want to orchestrate Trident servicing operations across your fleet?](#do-you-want-to-orchestrate-trident-servicing-operations-across-your-fleet)
    - [Contribute to Trident](#contribute-to-trident)
  - [Future developments](#future-developments)

## Motivation

Linux distributions typically provide two primary types of bootable images: an
installer image and a virtual machine (VM) image.

The installer image, typically distributed in ISO format, boots into a live
management operating system and can be executed in either attended or unattended
modes. During installation, users are able to configure disk partitioning,
select desired features, and set system parameters such as timezone and user
accounts. Behind the scenes, the installer automates essential tasks including
disk partitioning and formatting, package installation, and operating system
configuration. Upon completion, the installer reboots into the fully provisioned
runtime environment. This installation process is suitable for deploying Linux
distributions to both bare metal hosts and virtual machines.

Alternatively, if your goal is to run the Linux distribution within a virtual
machine, you can obtain a VM image and initiate the boot process immediately.
This approach enables rapid deployment and operation of the Linux environment.
However, configuration changes can only be applied after the initial
boot—typically using tools such as `cloud-init`—which may necessitate additional
reboots to fully realize the desired system state.

Regardless of the image type selected, ongoing servicing is essential to address
security vulnerabilities (CVEs) and apply updates. For environments with spare
resources, scale-out servicing can be performed by deploying a new OS version
onto a separate node—virtual or bare metal—and decommissioning the older
instance upon completion. However, this approach may not be feasible for larger
clusters or resource-constrained scenarios where spare capacity is unavailable
or cost-prohibitive.

In such cases, servicing can be achieved by shutting down the current OS
instance, replacing the OS disk (either by deploying a new VM image or rerunning
the installer), and booting into the updated OS. This process is time-consuming
and requires additional orchestration from the underlying infrastructure.

To minimize downtime and avoid reliance on spare resources, in-place servicing
is preferable. Traditional Linux distributions typically support package-based
updates, but these methods lack robust rollback capabilities and can result in
inconsistencies across nodes due to timing and package variations.

A more reliable approach is to use image-based A/B style in-place atomic
updates, similar to those used by Android. With [A/B updates](Reference/Glossary.md#ab-update), rollback is
straightforward—either during servicing or at any later point—without requiring
extra resources. Additionally, servicing downtime is reduced, as the B set of
images can be pre-staged while the A set remains operational.

Traditionally, Linux distributions provide distinct mechanisms for initial
installation and subsequent servicing of the operating system. Trident
streamlines this process by offering a unified workflow that seamlessly handles
both installation and ongoing servicing tasks.

Regardless of whether you are deploying to bare metal hosts or virtual machines,
and whether you utilize installer images or VM images, Trident delivers a
consistent, atomic approach to OS deployment and servicing. Its composable
architecture enables easy integration into broader solutions, eliminating the
need to manually coordinate low-level OS utilities for disk partitioning, image
installation, bootloader configuration, and system setup—Trident manages these
operations efficiently on your behalf.

## What is Trident?

Trident operates as a servicing agent, drawing inspiration from the declarative
API principles established by Kubernetes. It ingests a [Host Configuration
specification](Reference/Host-Configuration/HostConfiguration.md) as input, and, as it progresses, updates the Host Status to
accurately reflect all changes applied in accordance with the provided Host
Configuration.

The Host Configuration defines the intended state of the host that Trident
manages, serving as the authoritative specification from initial installation
(when applicable) through all subsequent servicing operations. The Host
Configuration API is designed to align closely with the Image Customizer Image
Configuration API, ensuring consistency across deployment and servicing
workflows.

Basic example of a Host Configuration:

```yaml
storage:
  disks:
  - id: os
    device: /dev/disk/by-path/pci-0000:00:1f.2-ata-2.0
    partitionTableType: gpt
    partitions:
    - id: esp
      type: esp
      size: 64M
    - id: root
      type: root
      size: 8G
  filesystems:
  - deviceId: esp
    mountPoint:
      path: /boot/efi
      options: umask=0077
  - deviceId: root
    mountPoint: /
image:
  url: file:///path/to/image.cosi
  sha384: ec9a9aa23f02b30f4ec6a168b9bc24733b652eeab4f8abc243630666a5e34cea1667c34313a13ec1564ac4871b80112f
```

The Host Status provides a snapshot of the current configuration as managed by
Trident. This enables Trident to accurately report the operational state to
users and facilitates precise determination of required changes when a new Host
Configuration is supplied.

Trident offers a streamlined abstraction layer over established upstream Linux
utilities, including `systemd-repart`, `mdadm`, `cryptsetup`, `grub2`,
`veritysetup`, and others. By integrating these proven tools, Trident delivers a
consistent and dependable servicing experience while minimizing complexity.
Developed in Rust, Trident benefits from enhanced memory safety and performance,
ensuring robust and efficient operation.

Trident is architected for seamless integration into larger solutions. Its
primary responsibility is single-host servicing, delegating orchestration
tasks—such as scheduling and input selection—to external logic. Trident is
intentionally modular; it can be used solely for image deployment or in
conjunction with other tools for OS configuration. However, maximum benefit is
achieved when Trident manages the entire servicing workflow.

Further, Trident is designed to be platform and product agnostic. This allows
the common servicing logic to be reused across various products and
environments, while product-specific logic is handled externally. This
separation of concerns simplifies maintenance and enables consistent servicing
practices across diverse deployments.

Trident is capable of operating in two distinct modes: it can execute from a
live management operating system to facilitate initial OS installation, or it
can run directly within the host OS to perform image-based A/B-style servicing
and updates.

Trident-based installer can be deployed through multiple mechanisms, including
bootable ISO images, PXE boot, or other provisioning tools. This flexibility
allows users to choose the most suitable method for their environment and
requirements.

Trident is capable of operating either directly within the host OS root
namespace or in a containerized environment. It can be initiated interactively,
by product-specific orchestration logic, or managed as a service via `systemd`.
When no servicing operations are pending, the Trident agent remains inactive,
ensuring minimal consumption of system resources.

## How does Trident work?

Trident operates based on two primary concepts: OS installation and update, each
corresponding to dedicated CLI commands. Upon invocation, Trident receives a
command along with a Host Configuration that defines the desired state of the
host. During initial installation—or when performing offline initialization for
a VM image—Trident establishes its own datastore (implemented as a SQLite
database) on the OS filesystem, capturing the current state via the Host Status
API.

For subsequent servicing operations, Trident compares the provided Host
Configuration against the most recent Host Status stored in its datastore. It
then queries its internal subsystems to determine the appropriate servicing type
required to reconcile the host with the desired configuration. Currently,
Trident supports the A/B update servicing model. In this model, Trident
pre-stages the OS image, applies additional OS configuration (including state
migration), updates the firmware boot configuration, and reboots into the newly
installed OS.

If any issues arise during servicing, Trident configures the firmware to
automatically roll back to the previous OS version, ensuring system reliability.
Once the deployment is verified as successful, the user or orchestrator can
certify the deployment, prompting Trident to mark it accordingly and update the
firmware configuration to reflect the new state.

The Trident datastore maintains a comprehensive record of all servicing
operations, including the Host Configuration and Host Status for each operation.
This helps to diagnose any issues that may arise during servicing and provides a
clear audit trail of changes made to the host over time.

Trident supports a two-phase servicing workflow: **stage** and **finalize**.
During the **stage** phase, updated OS images are pre-staged onto the host
without interrupting running workloads, enabling preparation for servicing with
minimal disruption. Once staging is complete, workloads can be gracefully
stopped, and the **finalize** phase is initiated. In this phase, Trident updates
the firmware configuration to select the appropriate OS version for boot, and
the host is rebooted to complete the servicing operation. While both phases can
be performed right after another, they can also be separated by an arbitrary
amount of time, allowing for flexible scheduling and coordination with other
maintenance tasks. This two-phase approach minimizes workload downtime and
ensures a smooth transition to the updated OS.

To stage OS images, Trident utilizes the COSI specification to enable efficient
transfer and reliable deployment of the OS images. Trident can query COSI
metadata without downloading the entire COSI file, then stream individual
filesystem images directly from the specified source—such as a local file, HTTPS
endpoint, or OCI registry—into the target block device (partition, RAID array,
LUKS volume, etc.). During this process, Trident performs on-the-fly hashing and
decompression of filesystem blocks, ensuring rapid transfer without requiring
additional storage or memory for intermediate placement.

Leveraging COSI metadata, Trident validates key aspects of the source images,
such as verifying that required dependencies are present, and provides precise
error reporting when issues are detected. Since COSI files can be generated as
an output format by Image Customizer, producing them is straightforward. Trident
employs COSI files for both OS installation and updates, with the source
location specified in the Host Configuration.

Trident allows users to preconfigure all required changes during the staging
phase. As a result, after reboot, the OS boots directly into its intended
state—identical to any subsequent boot—eliminating the need for special handling
of the initial boot or post-boot configuration using tools like `cloud-init`
(although such tools can still be used if desired).

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

Trident seamlessly integrates with Image Customizer, enabling it to retrieve
image configuration details from both the COSI file and the separate Image
History file. This approach minimizes duplication of configuration data between
build and deployment phases, streamlining the overall workflow and ensuring
consistency.

## How do I interact with Trident?

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

- `install`: Initiates the initial installation of the operating system.
- `offline-initialize`: Prepares the Trident datastore for a VM image, enabling
  future in-place servicing. This is typically performed during VM image
  creation.
- `update`: Executes an OS update in accordance with the supplied Host
  Configuration.
- `commit`: Certifies the current OS deployment as successful.
- `rebuild-raid`: Reconstructs a degraded software RAID array following physical
  drive replacement.
- `get`: Retrieves the most recent Host Configuration, Host Status, or error
  details. This is particularly useful for non-interactive scenarios.
- `validate`: Performs an offline validation of a Host Configuration to ensure
  it is well-formed and applicable. Note that this validation is host
  context-free and may not detect all potential issues.

Please consult [CLI reference](Reference/Trident-CLI.md) for detailed information on each command and its usage.

Trident is designed for both interactive use by administrators and
non-interactive integration with orchestration systems. In automated
environments, orchestrators can utilize the `get` command to monitor operation
status and determine appropriate next steps based on structured feedback from
Trident.

## What can Trident do?

Trident offers a comprehensive set of capabilities for OS installation and
servicing.

**Installation Features:**

- Disk partitioning and formatting using the GUID Partition Table (GPT).
- Creation of software RAID arrays, including support for ESP redundancy.
- Provisioning of encrypted volumes, with optional PCR sealing.
- DM-verity integration for root and `/usr` filesystems.
- Adoption of existing partitions and filesystems (preview).
- Multiboot support for side-by-side installation of multiple OS images
  (preview).

**Installation and Servicing Features:**

- Deployment of compressed, minimized OS images in COSI format from local files,
  HTTPS sources, or OCI registries.
- Bootloader configuration, supporting both `grub2` and `systemd-boot`.
- OS configuration management, including network settings, hostname, user
  accounts, SSH, and SELinux policies.
- Execution of user-provided scripts for custom OS image modifications.
- Reliable rollback to the previous OS version in case of servicing issues.
- Unified Kernel Image (UKI) support (preview).

Trident supports servicing both bare metal hosts and virtual machines.

Trident builds are available for `x86_64` and `aarch64` architectures.

## See a prerecorded demo of Trident in action

[![Trident Demo](https://img.youtube.com/vi/0/0.jpg)](https://www.youtube.com/watch?v=0)

## How can I get started?

### Found an issue or missing a feature?

If you found a bug or want to request a feature, please file an issue in the
[Trident GitHub repository](https://github.com/microsoft/trident/issues).

### Try out Trident

#### Do you want to deploy a bare metal host?

[Get started with bare metal deployment](Trident-BareMetal.md)

#### Do you want to update a bare metal host or a virtual machine?

[Get started with updates](Tutorials/Performing-an-ABUpdate.md).

#### Do you want to orchestrate Trident servicing operations across your fleet?

[Get started with orchestration](Trident-Orchestration.md).

### Contribute to Trident

Trident is an open source project and we welcome contributions. If you want to
contribute, please check out the [contributing
guide](https://github.com/microsoft/trident/blob/main/CONTRIBUTING.md).

## Future developments

Trident is under active development, with several enhancements planned for
future releases. Key areas of focus include:

- Support for servicing systemd System Extensions (sysexts).
- Enhanced SELinux policy management and updates.
- User-initiated rollback capabilities.
- Introduction of a gRPC API for improved integration.
- Implementation of a Host Report API to provide detailed hardware and software
  inventory.
- Addition of a pre-reboot hook for advanced servicing workflows.
