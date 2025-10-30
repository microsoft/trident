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
        config="azl-installer",
        config_file=Path("installer-iso.yaml"),
        output_format=OutputFormat.ISO,
        requires_trident=True,
        extra_dependencies=[
            Path("tests/images/azl-installer/iso/bin/liveinstaller"),
            Path("tests/images/azl-installer/iso/images/trident-testimage.cosi"),
        ],
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
