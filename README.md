---
ArtifactType: executable, rpm.
Documentation: ./README.md
Language: rust
Platform: mariner
Stackoverflow: URL
Tags: comma,separated,list,of,tags
---

# Trident

**Trident** is a declarative, security-first OS lifecycle agent designed primarily for [Azure Linux](https://github.com/microsoft/azurelinux/?tab=readme-ov-file#azure-linux). It supports clean installation and provisioning of bare-metal nodes, as well as A/B-style updates and runtime configuration for both bare-metal and virtual machines.

At the heart of Trident is its **Host Configuration API**, a declarative YAML-based interface that defines the desired state of a machine across storage, OS, networking, and firmware layers.

Trident supports a wide range of lifecycle and configuration capabilities:

- ✅ Disk partitioning and imaging  
- ✅ RAID configuration  
- ✅ Encrypted volumes with TPM/PCR support  
- ✅ dm-verity support  
- ✅ A/B update  
- ✅ Bootloader configuration  
- ✅ Networking configuration  
- ✅ User management  
- ✅ SELinux configuration  
- ✅ Custom hooks  
- ✅ ...and more


## Getting Started

### Documentation

Our [documentation](docs/Trident.md) is still underconstruction. For now, please use the [Getting Started Guide](GETTING_STARTED.md).

### Developing and Contributing

For detailed information about contributing to this project please read the
[contributing guide](./docs/Development/Contributing/contribuiting-guidelines.md).

## Getting Help

Have questions, found a bug, or need a new feature? Open an issue in our [GitHub
repository](https://github.com/microsoft/trident/issues/new?template=Blank+issue).

---

## Trademarks

This project may contain trademarks or logos for projects, products, or
services. Authorized use of Microsoft trademarks or logos is subject to and must
follow [Microsoft's Trademark & Brand
Guidelines](https://www.microsoft.com/en-us/legal/intellectualproperty/trademarks/usage/general).
Use of Microsoft trademarks or logos in modified versions of this project must
not cause confusion or imply Microsoft sponsorship. Any use of third-party
trademarks or logos are subject to those third-party's policies.

