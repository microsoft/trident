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
        "trident-functest",
        output_format=OutputFormat.QCOW2,
        requires_trident=False,
    ),
    ImageConfig(
        "azl-installer",
        config_file=Path("installer-iso.yaml"),
        output_format=OutputFormat.ISO,
        requires_trident=True,
        extra_dependencies=[
            Path("tests/images/azl-installer/iso/bin/liveinstaller"),
            Path("tests/images/azl-installer/iso/images/trident-testimage.cosi"),
        ],
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
