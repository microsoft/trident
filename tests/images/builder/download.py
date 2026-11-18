import hashlib
from pathlib import Path
import shutil
import subprocess
import tempfile

from builder import (
    BaseImageSource,
    PipelineArtifactBaseImageManifest,
)


def download_base_image(image: BaseImageSource) -> None:
    """Download a base image from MCR or a pinned pipeline artifact."""
    if isinstance(image, PipelineArtifactBaseImageManifest):
        download_pipeline_artifact(image)
        return

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


def download_pipeline_artifact(image: PipelineArtifactBaseImageManifest) -> None:
    image.image.path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=image.image.path.parent) as tempdir:
        subprocess.run(
            [
                "az",
                "pipelines",
                "runs",
                "artifact",
                "download",
                "--organization",
                image.org,
                "--project",
                image.project,
                "--run-id",
                str(image.run_id),
                "--artifact-name",
                image.artifact_name,
                "--path",
                tempdir,
            ],
            check=True,
        )
        matches = list(Path(tempdir).rglob(image.filename))
        if len(matches) != 1 or not matches[0].is_file():
            raise RuntimeError(
                f"Expected exactly one {image.filename} in pipeline run "
                f"{image.run_id}, found {len(matches)}"
            )
        checksum = hashlib.sha256()
        with matches[0].open("rb") as source:
            for chunk in iter(lambda: source.read(1024 * 1024), b""):
                checksum.update(chunk)
        digest = checksum.hexdigest()
        if digest != image.sha256:
            raise RuntimeError(
                f"SHA256 mismatch for {image.filename} from pipeline run "
                f"{image.run_id}: expected {image.sha256}, got {digest}"
            )
        matches[0].replace(image.image.path)
