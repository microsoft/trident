import json
import logging
from pathlib import Path
import shutil
import subprocess
import tempfile

from builder import (
    BaseImageSource,
    PipelineArtifactBaseImageManifest,
)


def download_base_image(image: BaseImageSource) -> None:
    """Download a base image from MCR or a successful scheduled pipeline run."""
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
