---
sidebar_position: 6
---

# Future developments

Trident is under active development, with several enhancements planned for
future releases. Key areas of focus include:

- Support for servicing systemd System Extensions (sysexts) and Configuration
  Extensions (confexts). See [Sysexts](../Explanation/Sysexts.md) and
  [Confexts](../Explanation/Confexts.md). ✅
- User-initiated rollback capabilities. See
  [Manual Rollback](../Explanation/Manual-Rollback.md). ✅
- Introduction of a gRPC API for improved integration. See
  [gRPC Server](../Explanation/gRPC-Server.md). ✅
- Runtime updates for sysexts, confexts, and network configuration without
  reboot. See [Runtime Updates](../Explanation/Runtime-Updates.md). ✅
- Disk streaming for fast provisioning directly from COSI images. See
  [Disk Streaming](../Explanation/Disk-Streaming.md). ✅
- Image streaming from OCI registries. See
  [Image Streaming Pipeline](../Explanation/Image-Streaming-Pipeline.md). ✅
- Enhanced SELinux policy management and updates.
- Implementation of a Host Report API to provide detailed hardware and software
  inventory.
- Addition of a pre-reboot hook for advanced servicing workflows.
- Kexec support to enable faster reboots during servicing operations.
