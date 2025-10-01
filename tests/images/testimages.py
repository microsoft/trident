#!/usr/bin/env python3

from pathlib import Path
from typing import List

from builder import (
    ArtifactManifest,
    BaseImage,
    BaseImageManifest,
    ImageConfig,
    OutputFormat,
    SystemArchitecture,
    cli,
)

# # # # # # # # # # # # # # # # # # #
#    DEFAULT CUSTOMIZER VERSION     #
#                                   #
# The version of Image Customizer   #
# to use by default when building   #
# images.                           #
# # # # # # # # # # # # # # # # # # #
DEFAULT_IMAGE_CUSTOMIZER_VERSION = "0.19"


DEFINED_IMAGES: List[ImageConfig] = [
    ImageConfig(
        "trident-installer",
        config="trident-installer",
        output_format=OutputFormat.ISO,
    ),
    ImageConfig(
        "trident-split-installer",
        config="trident-installer",
        config_file="base/baseimg-split.yaml",
        output_format=OutputFormat.ISO,
    ),
    ImageConfig(
        "trident-installer-arm64",
        config="trident-installer",
        output_format=OutputFormat.ISO,
        base_image=BaseImage.CORE_ARM64,
        architecture=SystemArchitecture.ARM64,
    ),
    ImageConfig(
        "trident-container-installer",
        config="trident-container-installer",
        output_format=OutputFormat.ISO,
    ),
    ImageConfig(
        "trident-functest",
        output_format=OutputFormat.QCOW2,
        requires_trident=False,
    ),
    ImageConfig("trident-testimage"),
    ImageConfig(
        "trident-testimage-arm64",
        config="trident-testimage",
        base_image=BaseImage.CORE_ARM64,
        architecture=SystemArchitecture.ARM64,
    ),
    ImageConfig("trident-verity-testimage"),
    ImageConfig(
        "trident-usrverity-testimage",
        config="trident-verity-testimage",
        config_file="usr/host.yaml",
        requires_ukify=True,
    ),
    ImageConfig(
        "trident-container-verity-testimage",
        config="trident-verity-testimage",
        config_file="base/baseimg-container.yaml",
        requires_trident=False,
    ),
    ImageConfig(
        "trident-container-usrverity-testimage",
        config="trident-verity-testimage",
        config_file="usr/container.yaml",
        requires_ukify=True,
        requires_trident=False,
    ),
    ImageConfig(
        "trident-container-testimage",
        requires_trident=False,
    ),
    ImageConfig(
        "trident-vm-grub-testimage",
        base_image=BaseImage.QEMU_GUEST,
        config="trident-vm-testimage",
        config_file="base/updateimg-grub.yaml",
        ssh_key="files/id_rsa.pub",
    ),
    ImageConfig(
        "trident-vm-grub-verity-testimage",
        base_image=BaseImage.QEMU_GUEST,
        config="trident-vm-testimage",
        config_file="base/updateimg-grub-verity.yaml",
        ssh_key="files/id_rsa.pub",
    ),
    ImageConfig(
        "trident-vm-root-verity-testimage",
        base_image=BaseImage.QEMU_GUEST,
        config="trident-vm-testimage",
        config_file="base/baseimg-root-verity.yaml",
        requires_ukify=True,
        ssh_key="files/id_rsa.pub",
    ),
    ImageConfig(
        "trident-vm-usr-verity-testimage",
        base_image=BaseImage.QEMU_GUEST,
        config="trident-vm-testimage",
        config_file="base/baseimg-usr-verity.yaml",
        requires_ukify=True,
        ssh_key="files/id_rsa.pub",
    ),
    ImageConfig(
        "trident-vm-grub-verity-azure-testimage",
        base_image=BaseImage.CORE_SELINUX,
        config="trident-vm-testimage",
        config_file="base/updateimg-grub-verity-azure.yaml",
    ),
    ImageConfig(
        "trident-vm-grub-testimage-arm64",
        base_image=BaseImage.CORE_ARM64,
        config="trident-vm-testimage",
        config_file="base/updateimg-grub.yaml",
        ssh_key="files/id_rsa.pub",
        architecture=SystemArchitecture.ARM64,
    ),
    ImageConfig(
        "trident-vm-grub-verity-testimage-arm64",
        base_image=BaseImage.CORE_ARM64,
        config="trident-vm-testimage",
        config_file="base/updateimg-grub-verity.yaml",
        ssh_key="files/id_rsa.pub",
        architecture=SystemArchitecture.ARM64,
    ),
    ImageConfig(
        "pxe-server",
        base_image=BaseImage.MINIMAL,
        config="pxe-server",
        config_file="pxe-server.yaml",
        ssh_key="files/id_rsa.pub",
        requires_dhcp=True,
    ),
]

ARTIFACTS = ArtifactManifest(
    customizer_version=DEFAULT_IMAGE_CUSTOMIZER_VERSION,
    customizer_container="mcr.microsoft.com/azurelinux/imagecustomizer",
    base_images=[
        BaseImageManifest(
            image=BaseImage.BAREMETAL,
            package_name="baremetal_vhdx-3.0-stable",
            version="*",
        ),
        BaseImageManifest(
            image=BaseImage.CORE_SELINUX,
            package_name="core_selinux_vhdx-3.0-stable",
            version="*",
        ),
        BaseImageManifest(
            image=BaseImage.QEMU_GUEST,
            package_name="qemu_guest_vhdx-3.0-stable",
            version="*",
        ),
        BaseImageManifest(
            image=BaseImage.MINIMAL,
            package_name="minimal_vhdx-3.0-stable",
            version="*",
        ),
    ],
)

if __name__ == "__main__":
    import os

    # Change to the base directory in the trident repo
    os.chdir(Path(__file__).parent.parent.parent)

    # Run the CLI
    cli.init(DEFINED_IMAGES, ARTIFACTS)
