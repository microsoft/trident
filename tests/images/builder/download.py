import json
import logging
from pathlib import Path
import shutil
import subprocess
import tempfile

from builder import BaseImageManifest, BlobImageManifest

log = logging.getLogger(__name__)


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


def download_blob_image(
    image: BlobImageManifest,
    storage_account: str,
    container: str,
) -> None:
    """Download a base image from Azure Storage Blob.

    Lists blobs under `image.path_prefix`, filters to ones ending with
    `image.file_suffix`, picks the lexically largest (= most recent
    version), and downloads it to `image.image.path`.

    Requires `az` CLI with a logged-in identity that has read access
    to the storage account. Uses `--auth-mode login` so no storage
    keys are needed.
    """
    if not storage_account or not container:
        raise RuntimeError(
            f"Blob storage account/container required to download "
            f"'{image.image.name}'. Pass --blob-storage-account and "
            f"--blob-container, or set BLOB_STORAGE_ACCOUNT and "
            f"BLOB_CONTAINER env vars."
        )

    az = shutil.which("az")
    if az is None:
        raise RuntimeError(
            "az CLI not found on PATH; required to fetch blob-sourced "
            "base images. Install azure-cli."
        )

    log.info(
        f"Listing blobs in '{storage_account}/{container}' under "
        f"prefix '{image.path_prefix}/'"
    )
    list_proc = subprocess.run(
        [
            az,
            "storage",
            "blob",
            "list",
            "--auth-mode",
            "login",
            "--account-name",
            storage_account,
            "--container-name",
            container,
            "--prefix",
            f"{image.path_prefix}/",
            "--query",
            f"[?ends_with(name, '{image.file_suffix}')].name",
            "-o",
            "json",
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    names = json.loads(list_proc.stdout)
    if not names:
        raise RuntimeError(
            f"No blobs ending with '{image.file_suffix}' found under "
            f"'{image.path_prefix}/' in '{storage_account}/{container}'"
        )

    latest = sorted(names)[-1]
    log.info(f"Latest: {latest}")

    image.image.path.parent.mkdir(parents=True, exist_ok=True)

    subprocess.run(
        [
            az,
            "storage",
            "blob",
            "download",
            "--auth-mode",
            "login",
            "--account-name",
            storage_account,
            "--container-name",
            container,
            "--name",
            latest,
            "--file",
            str(image.image.path),
            "--output",
            "none",
        ],
        check=True,
    )
