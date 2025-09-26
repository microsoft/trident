---
sidebar_position: 1
---

# What is Trident?

Trident is a servicing agent, drawing inspiration from the declarative API
principles established by Kubernetes. It ingests a [**Host Configuration**
specification](Reference/Host-Configuration/API-Reference/HostConfiguration.md)
as input, and, as it progresses, updates the **Host Status** to accurately
reflect all changes applied in accordance with the provided Host Configuration.

## Host Configuration

The Host Configuration defines the desired state of the host that Trident
manages, serving as the authoritative specification from initial installation
(when applicable) through all subsequent servicing operations. The Host
Configuration API is designed to align closely with the [Image
Customizer](https://microsoft.github.io/azure-linux-image-tools/imagecustomizer/README.html)
[Image Configuration
API](https://microsoft.github.io/azure-linux-image-tools/imagecustomizer/api/configuration.html),
ensuring consistency across deployment and servicing workflows.

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

## Host Status

The Host Status provides a snapshot of the current configuration as managed by
Trident. This enables Trident to accurately report the operational state to
users and facilitates precise determination of required changes when a new Host
Configuration is supplied.

## Simplifying complexity through integration and reuse

Trident offers a streamlined abstraction layer over established upstream Linux
utilities, including `systemd-repart`, `mdadm`, `cryptsetup`, `veritysetup`, and
others and leverages standard upstream components such as `grub2` and
`systemd-boot`. By integrating these proven tools, Trident delivers a consistent
and dependable servicing experience while minimizing complexity. Developed in
Rust, Trident benefits from enhanced memory safety and performance, ensuring
robust and efficient operation.

## Architectural principles

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

## Operating modes

Trident is capable of operating in two distinct modes: it can execute from a
live management operating system to facilitate initial OS installation, or it
can run directly within the host OS to perform image-based A/B-style servicing
and updates.

Trident-based installer can be deployed through multiple mechanisms, including
bootable ISO images, PXE boot, or other provisioning tools. This flexibility
allows users to choose the most suitable method for their environment and
requirements.

Trident is capable of operating either directly within the host OS root
namespace or in a [containerized
environment](How-To-Guides/Run-Trident-Inside-a-Container.md). It can be
initiated interactively, by product-specific orchestration logic, or managed as
a service via `systemd`. When no servicing operations are pending, the Trident
agent remains inactive, ensuring minimal consumption of system resources.
