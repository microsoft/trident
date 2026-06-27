#!/usr/bin/env python3

from pathlib import Path
from typing import List

from builder import (
    ArtifactManifest,
    BaseImage,
    BaseImageManifest,
    BlobImageManifest,
    Distro,
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
DEFAULT_IMAGE_CUSTOMIZER_VERSION = "latest"


DEFINED_IMAGES: List[ImageConfig] = [
    # Installer images
    ImageConfig(
        "trident-installer",
        config="trident-installer",
        output_and_config={OutputFormat.ISO: "base/baseimg.yaml"},
    ),
    ImageConfig(
        "trident-split-installer",
        config="trident-installer",
        output_and_config={OutputFormat.ISO: "base/baseimg-split.yaml"},
    ),
    ImageConfig(
        "trident-installer-arm64",
        config="trident-installer",
        output_and_config={OutputFormat.ISO: "base/baseimg.yaml"},
        base_image=BaseImage.CORE_ARM64,
        architecture=SystemArchitecture.ARM64,
    ),
    ImageConfig(
        "trident-container-installer",
        config="trident-container-installer",
        output_and_config={OutputFormat.ISO: "base/baseimg.yaml"},
        requires_trident=False,
    ),
    ImageConfig(
        "trident-direct-streaming-installer-amd64",
        config="trident-installer",
        output_and_config={OutputFormat.ISO: "base/baseimg-direct-streaming.yaml"},
    ),
    ImageConfig(
        "trident-direct-streaming-installer-arm64",
        config="trident-installer",
        output_and_config={OutputFormat.ISO: "base/baseimg-direct-streaming.yaml"},
        base_image=BaseImage.CORE_ARM64,
        architecture=SystemArchitecture.ARM64,
    ),
    # Test images
    ImageConfig(
        "trident-functest",
        output_and_config={OutputFormat.QCOW2: "base/baseimg.yaml"},
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
        output_and_config={OutputFormat.COSI: "usr/host.yaml"},
        requires_ukify=True,
    ),
    ImageConfig(
        "trident-container-verity-testimage",
        config="trident-verity-testimage",
        output_and_config={OutputFormat.COSI: "base/baseimg-container.yaml"},
        requires_trident=False,
    ),
    ImageConfig(
        "trident-container-usrverity-testimage",
        config="trident-verity-testimage",
        output_and_config={OutputFormat.COSI: "usr/container.yaml"},
        requires_ukify=True,
        requires_trident=False,
    ),
    ImageConfig(
        "trident-container-testimage",
        requires_trident=False,
    ),
    # Direct streaming images
    ImageConfig(
        "azurelinux-direct-streaming-testimage-amd64",
        config="azurelinux-direct-streaming-testimage",
        output_and_config={OutputFormat.BAREMETAL_IMAGE: "base/baseimg.yaml"},
    ),
    ImageConfig(
        "azurelinux-direct-streaming-testimage-arm64",
        config="azurelinux-direct-streaming-testimage",
        output_and_config={OutputFormat.BAREMETAL_IMAGE: "base/baseimg.yaml"},
        base_image=BaseImage.CORE_ARM64,
        architecture=SystemArchitecture.ARM64,
    ),
    # AZL installer
    ImageConfig(
        "azl-installer",
        output_and_config={OutputFormat.ISO: "installer-iso.yaml"},
        requires_trident=True,
        extra_dependencies=[
            Path("tests/images/azl-installer/iso/bin/liveinstaller"),
            Path("tests/images/azl-installer/iso/images/trident-testimage.cosi"),
        ],
    ),
    # VM test images (azl3)
    ImageConfig(
        "trident-vm-grub-testimage",
        base_image=BaseImage.QEMU_GUEST,
        config="trident-vm-testimage",
        output_and_config={
            OutputFormat.COSI: "base/updateimg-grub.yaml",
            OutputFormat.QCOW2: "base/baseimg-grub.yaml",
        },
        ssh_key="files/id_rsa.pub",
    ),
    ImageConfig(
        "trident-vm-grub-verity-testimage",
        base_image=BaseImage.QEMU_GUEST,
        config="trident-vm-testimage",
        output_and_config={
            OutputFormat.COSI: "base/updateimg-grub-verity.yaml",
            OutputFormat.QCOW2: "base/baseimg-grub-verity.yaml",
        },
        ssh_key="files/id_rsa.pub",
    ),
    ImageConfig(
        "trident-vm-root-verity-testimage",
        base_image=BaseImage.QEMU_GUEST,
        config="trident-vm-testimage",
        output_and_config={
            OutputFormat.COSI: "base/baseimg-root-verity.yaml",
            OutputFormat.QCOW2: "base/baseimg-root-verity.yaml",
        },
        requires_ukify=True,
        ssh_key="files/id_rsa.pub",
    ),
    ImageConfig(
        "trident-vm-usr-verity-testimage",
        base_image=BaseImage.QEMU_GUEST,
        config="trident-vm-testimage",
        output_and_config={
            OutputFormat.COSI: "base/baseimg-usr-verity.yaml",
            OutputFormat.QCOW2: "base/baseimg-usr-verity.yaml",
        },
        requires_ukify=True,
        ssh_key="files/id_rsa.pub",
    ),
    ImageConfig(
        "trident-vm-grub-verity-azure-testimage",
        base_image=BaseImage.CORE_SELINUX,
        config="trident-vm-testimage",
        output_and_config={
            OutputFormat.COSI: "base/updateimg-grub-verity-azure.yaml",
            OutputFormat.QCOW2: "base/baseimg-grub-verity-azure.yaml",
            OutputFormat.VHD_FIXED: "base/baseimg-grub-verity-azure.yaml",
        },
    ),
    ImageConfig(
        "trident-vm-grub-testimage-arm64",
        base_image=BaseImage.CORE_ARM64,
        config="trident-vm-testimage",
        output_and_config={
            OutputFormat.COSI: "base/updateimg-grub.yaml",
            OutputFormat.QCOW2: "base/baseimg-grub.yaml",
        },
        ssh_key="files/id_rsa.pub",
        architecture=SystemArchitecture.ARM64,
    ),
    ImageConfig(
        "trident-vm-grub-verity-testimage-arm64",
        base_image=BaseImage.CORE_ARM64,
        config="trident-vm-testimage",
        output_and_config={
            OutputFormat.COSI: "base/updateimg-grub-verity.yaml",
            OutputFormat.QCOW2: "base/baseimg-grub-verity.yaml",
        },
        ssh_key="files/id_rsa.pub",
        architecture=SystemArchitecture.ARM64,
    ),
    # VM test images (azl4)
    ImageConfig(
        "trident-vm-grub-azl4-testimage",
        base_image=BaseImage.AZL4_QEMU_GUEST,
        config="trident-vm-testimage",
        output_and_config={
            OutputFormat.COSI: "base/updateimg-grub-azl4.yaml",
            OutputFormat.QCOW2: "base/baseimg-grub-azl4.yaml",
        },
        ssh_key="files/id_rsa.pub",
    ),
    # stream-image test images
    ImageConfig(
        "ubuntu-direct-streaming-testimage-2204-amd64",
        base_image=BaseImage.UBUNTU_2204_AMD64,
        output_and_config={OutputFormat.BAREMETAL_IMAGE: "base/baseimg.yaml"},
        image_customizer_convert=True,
        requires_trident=False,
    ),
    ImageConfig(
        "ubuntu-direct-streaming-testimage-2204-arm64",
        base_image=BaseImage.UBUNTU_2204_ARM64,
        output_and_config={OutputFormat.BAREMETAL_IMAGE: "base/baseimg.yaml"},
        architecture=SystemArchitecture.ARM64,
        image_customizer_convert=True,
        requires_trident=False,
    ),
    ImageConfig(
        "ubuntu-direct-streaming-testimage-2404-amd64",
        base_image=BaseImage.UBUNTU_2404_AMD64,
        output_and_config={OutputFormat.BAREMETAL_IMAGE: "base/baseimg.yaml"},
        image_customizer_convert=True,
        requires_trident=False,
    ),
    ImageConfig(
        "ubuntu-direct-streaming-testimage-2404-arm64",
        base_image=BaseImage.UBUNTU_2404_ARM64,
        output_and_config={OutputFormat.BAREMETAL_IMAGE: "base/baseimg.yaml"},
        architecture=SystemArchitecture.ARM64,
        image_customizer_convert=True,
        requires_trident=False,
    ),
    ImageConfig(
        "gb200-direct-streaming-testimage-2404-arm64",
        base_image=BaseImage.GB200_2404_ARM64,
        output_and_config={OutputFormat.BAREMETAL_IMAGE: "base/baseimg.yaml"},
        architecture=SystemArchitecture.ARM64,
        image_customizer_convert=True,
        requires_trident=False,
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
            image=BaseImage.CORE_ARM64,
            package_name="core_vhdx-arm64-3.0-stable",
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
        # BaseImageManifest(
        #     image=BaseImage.AZL4_CORE,
        #     package_name="core_vhdx-4.0-stable",
        #     version="*",
        #     distro=Distro.AZL4,
        # ),
        BlobImageManifest(
            # Azure Linux 4.0 base VHDX from the AZL preview gallery's
            # backing storage. Pinned to a specific daily build — bump
            # the version segment in path_prefix to pick up a newer one.
            #
            # Source gallery:
            #   azlpubDevGallery2mruiyvi / azure-linux-4-daily-x64
            #   subscription e4ab81f8-030f-4593-a8f2-3ea2c7630a19
            #   RG azl-acg-preview-publishing
            #
            # Storage account + container are supplied at runtime via
            # --blob-storage-account / --blob-container CLI flags or
            # the BLOB_STORAGE_ACCOUNT / BLOB_CONTAINER env vars.
            image=BaseImage.AZL4_QEMU_GUEST,
            path_prefix="staging/azure-linux-4-daily-x64/4.0.2026051502",
            file_suffix=".vhdfixed",
        ),
    ],
)

if __name__ == "__main__":
    import os

    # Change to the base directory in the trident repo
    os.chdir(Path(__file__).parent.parent.parent)

    # Run the CLI
    cli.init(DEFINED_IMAGES, ARTIFACTS)
