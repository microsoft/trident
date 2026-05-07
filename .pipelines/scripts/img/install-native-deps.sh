#!/bin/bash
# Install native dependencies needed for building test images.
# Expected env vars: MIC_ARCHITECTURE, PIPELINE_ARCHITECTURE
set -eux

if which tdnf; then
  # nss-tools provides certutil while pesign provides efikeygen & pesign,
  # which are required for producing a signed image to enable SecureBoot
  sudo tdnf install -y veritysetup squashfs-tools lsof nss-tools pesign
  sudo systemctl start docker
else
  # Ubuntu is used for building and testing of VM images suitable for
  # servicing by Trident
  sudo apt install -y createrepo-c swtpm squashfs-tools lsof
fi

if [[ "${MIC_ARCHITECTURE}" != "${PIPELINE_ARCHITECTURE}" ]]; then
  # Register qemu for cross-platform ImageCustomizer usage
  if which tdnf; then
    sudo tdnf install -y qemu-user-static
  else
    sudo apt install -y qemu-user-static binfmt-support
  fi
fi

az extension add --name azure-devops
