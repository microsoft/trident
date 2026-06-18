import json
import logging
import os
import re
from pathlib import Path
import shutil
import subprocess
import tempfile

from builder import BaseImageManifest, BlobImageManifest, Distro

log = logging.getLogger(__name__)


def download_base_image(image: BaseImageManifest) -> None:
    if image.distro not in (Distro.AZL3, Distro.AZL4):
        raise ValueError(f"Unsupported distro {image.distro} for base image download")
    """Download the base image from MCR."""
    with tempfile.TemporaryDirectory() as tempdir:
        url = (
            f"mcr.microsoft.com/azurelinux-beta/base/{image.image.mcr_name}:4.0"
            if image.distro == Distro.AZL4
            else f"mcr.microsoft.com/azurelinux/3.0/image/{image.image.name}:latest"
        )
        subprocess.run(
            [
                "oras",
                "pull",
                url,
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


# Constrain blob filename selection to a date-prefixed shape so a stray
# blob with a name that lexically sorts last (`zzz-evil/image.vhdx`)
# cannot win selection. Matches `YYYYMMDD/` or `YYYY-MM-DD/`-style
# version prefixes, which is the upstream publisher's convention.
#
# This is defense against a broader governance issue: the storage account
# is owned by another team, so write access is out of Trident's control.
# The regex narrows the attack surface to "names matching this shape"
# while still letting us track the latest published version. Tracked
# longer-term in the AZL4 supply-chain governance discussion.
_BLOB_NAME_VERSION_RE = re.compile(r"/([^/]*\d{4}-?\d{2}-?\d{2}[^/]*)/")


def download_blob_image(
    image: BlobImageManifest,
    storage_account: str,
    container: str,
) -> None:
    """Download a base image from Azure Storage Blob.

    Lists blobs under `image.path_prefix`, filters to ones whose name
    matches a date-prefixed version pattern AND ends with
    `image.file_suffix`, picks the lexically largest (= most recent
    date), and downloads it atomically to `image.image.path`.

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
    # No `--query` interpolation: do the filtering in Python so caller
    # control of `image.file_suffix` (or any other field that might
    # become externally settable later) cannot inject JMESPath.
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
            "[].name",
            "-o",
            "json",
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    all_names = json.loads(list_proc.stdout)
    suffix = image.file_suffix
    eligible = [
        n for n in all_names if n.endswith(suffix) and _BLOB_NAME_VERSION_RE.search(n)
    ]
    if not eligible:
        raise RuntimeError(
            f"No date-versioned blobs ending with '{suffix}' found under "
            f"'{image.path_prefix}/' in '{storage_account}/{container}' "
            f"(saw {len(all_names)} total blobs under the prefix)"
        )

    latest = sorted(eligible)[-1]
    log.info(f"Latest: {latest}")

    image.image.path.parent.mkdir(parents=True, exist_ok=True)

    # Download to a sibling temp file then atomically rename. `az
    # storage blob download` writes in place — if the step is killed
    # (timeout / OOM / agent reboot) between create and complete, the
    # next run sees a truncated VHDX and MIC fails with an opaque
    # error. The temp-then-rename pattern guarantees the target either
    # has the full bytes or doesn't exist.
    target = image.image.path
    fd, tmp_path = tempfile.mkstemp(
        prefix=target.name + ".",
        suffix=".part",
        dir=str(target.parent),
    )
    os.close(fd)
    try:
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
                tmp_path,
                "--output",
                "none",
            ],
            check=True,
        )
        os.replace(tmp_path, target)
    except BaseException:
        # On any failure, remove the temp file so we don't leave
        # partial-state debris next to the final path.
        try:
            os.unlink(tmp_path)
        except FileNotFoundError:
            pass
        raise
