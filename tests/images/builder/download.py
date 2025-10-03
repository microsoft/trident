from pathlib import Path
import shutil
import subprocess
import tempfile

from builder import BaseImageManifest


def download_base_image(image: BaseImageManifest) -> None:
    """Download the base image from MCR."""
    with tempfile.TemporaryDirectory() as tempdir:
        subprocess.run(
            [
                "oras",
                "pull",
                f"mcr.microsoft.com/azurelinux/3.0/image/{image.image.name}:latest",
                "--output",
                tempdir,
                "--platform",
                "linux/amd64",
            ],
            check=True,
        )

        # Find and copy the .vhdx file to the target location
        tempdir_path = Path(tempdir)
        vhdx_files = list(tempdir_path.glob("*.vhdx"))
        if not vhdx_files:
            raise RuntimeError(
                f"No .vhdx file found in downloaded image for {image.image.name}"
            )
        if len(vhdx_files) > 1:
            raise RuntimeError(
                f"Multiple .vhdx files found in downloaded image for {image.image.name}"
            )

        # Ensure the parent directory exists
        image.image.path.parent.mkdir(parents=True, exist_ok=True)

        # Copy the .vhdx file to the target location
        shutil.copy2(vhdx_files[0], image.image.path)
