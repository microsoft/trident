import json
import logging
import lzma
import os
import re
from pathlib import Path
import shutil
import subprocess
import tempfile
from typing import Optional

from builder import (
    BaseImageSource,
    BlobImageManifest,
    Distro,
    PipelineArtifactBaseImageManifest,
)


log = logging.getLogger(__name__)


def download_base_image(image: BaseImageSource) -> None:
    """Download a base image from MCR or a successful scheduled pipeline run."""
    if isinstance(image, PipelineArtifactBaseImageManifest):
        download_pipeline_artifact(image)
        return

    if image.distro not in (Distro.AZL3, Distro.AZL4):
        raise ValueError(f"Unsupported distro {image.distro} for base image download")
    target = image.image.path
    target.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=target.parent) as tempdir:
        url = (
            f"mcr.microsoft.com/azurelinux/catalog/{image.package_name}:{image.version}"
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

        tempdir_path = Path(tempdir)
        vhdx_files = list(tempdir_path.glob("*.vhdx")) + list(
            tempdir_path.glob("*.vhdx.xz")
        )
        if not vhdx_files:
            raise RuntimeError(
                f"No .vhdx file found in downloaded image for {image.image.name}"
            )
        if len(vhdx_files) > 1:
            raise RuntimeError(
                f"Multiple .vhdx files found in downloaded image for {image.image.name}"
            )

        source = vhdx_files[0]
        if source.suffix == ".xz":
            expanded = source.with_suffix("")
            with lzma.open(source, "rb") as compressed, expanded.open("wb") as output:
                shutil.copyfileobj(compressed, output)
            source = expanded
        source.replace(target)


def _az_json(image: PipelineArtifactBaseImageManifest, args: list):
    return json.loads(
        subprocess.check_output(
            [
                "az",
                *args,
                "--organization",
                image.org,
                "--output",
                "json",
                "--only-show-errors",
            ],
            text=True,
        )
    )


def select_pipeline_run(image: PipelineArtifactBaseImageManifest) -> dict:
    """Find the newest successful scheduled run matching all repository branches."""
    # The list command's branch filter applies only to the pipeline's own checkout.
    branch = image.repository_refs["self"]
    builds = _az_json(
        image,
        [
            "pipelines",
            "runs",
            "list",
            "--project",
            image.project,
            "--pipeline-ids",
            str(image.pipeline_id),
            "--branch",
            branch,
            "--reason",
            "schedule",
            "--status",
            "completed",
            "--result",
            "succeeded",
            "--query-order",
            "FinishTimeDesc",
            "--query",
            f"[?repository.name=='{image.repository_name}']."
            "{id: id, buildNumber: buildNumber}",
        ],
    )
    for build in builds:
        # Resolved resource branches require the per-run API. Keep inspecting
        # newest-first until both the pipeline and its source repositories match.
        run = _az_json(
            image,
            [
                "devops",
                "invoke",
                "--area",
                "pipelines",
                "--resource",
                "runs",
                "--route-parameters",
                f"project={image.project}",
                f"pipelineId={image.pipeline_id}",
                f"runId={build['id']}",
                "--api-version",
                "7.1",
            ],
        )
        repositories = run.get("resources", {}).get("repositories", {})
        if (
            run.get("id") != build["id"]
            or run.get("pipeline", {}).get("id") != image.pipeline_id
            or run.get("state") != "completed"
            or run.get("result") != "succeeded"
            or any(
                repositories.get(name, {}).get("refName") != ref
                for name, ref in image.repository_refs.items()
            )
        ):
            continue
        return build
    raise RuntimeError(
        f"No successful scheduled run returned for pipeline {image.pipeline_id} "
        f"with repository {image.repository_name} and refs {image.repository_refs}"
    )


def download_pipeline_artifact(image: PipelineArtifactBaseImageManifest) -> None:
    build = select_pipeline_run(image)
    run_id = build["id"]
    logging.getLogger(__name__).info(
        "Selected scheduled pipeline %s run %s (%s)",
        image.pipeline_id,
        run_id,
        build["buildNumber"],
    )
    image.image.path.parent.mkdir(parents=True, exist_ok=True)
    # Preserve the cached image until download and unique-file validation succeed.
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
                str(run_id),
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
                f"{run_id}, found {len(matches)}"
            )
        matches[0].replace(image.image.path)


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
    storage_account: Optional[str],
    container: Optional[str],
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
