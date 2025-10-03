from pathlib import Path
import shutil
import subprocess
import tempfile

from builder import BaseImageManifest


def az_cli_artifact_download(
    package_name: str,
    package_version: str,
    output_path: Path,
    org: str,
    project: str,
    feed: str,
    scope: str = "project",
) -> None:
    subprocess.run(
        [
            "az",
            "artifacts",
            "universal",
            "download",
            "--organization",
            org,
            "--project",
            project,
            "--scope",
            scope,
            "--feed",
            feed,
            "--name",
            package_name,
            "--version",
            package_version,
            "--path",
            output_path,
        ],
        check=True,
    )


def download_single(
    package_name: str,
    package_version: str,
    output_file: Path,
    download_filename_glob: str,
    org: str,
    project: str,
    feed: str,
    scope: str = "project",
) -> None:
    """Download a single file from Azure DevOps artifacts."""
    with tempfile.TemporaryDirectory(prefix="azcli-artifact-") as temp_dir:
        temp_dir_path = Path(temp_dir)
        az_cli_artifact_download(
            package_name=package_name,
            package_version=package_version,
            output_path=temp_dir_path,
            org=org,
            project=project,
            scope=scope,
            feed=feed,
        )

        # Find the downloaded file
        for file in temp_dir_path.glob(download_filename_glob):
            if file.is_file():
                shutil.move(file, output_file)
                return

        raise FileNotFoundError(f"File matching '{download_filename_glob}' not found.")


def download_base_image(image: BaseImageManifest) -> None:
    """Download the base image from Azure DevOps artifacts."""
    # download_single(
    #     package_name=image.package_name,
    #     package_version=image.version,
    #     output_file=image.image.path,
    #     download_filename_glob=image.glob,
    #     org=image.org,
    #     project=image.project,
    #     feed=image.feed,
    # )
    subprocess.run(
        [
            "oras",
            "pull",
            f"mcr.microsoft.com/azurelinux/3.0/image/{image.image.name}:latest",
            "--output",
            image.image.path,
        ],
        check=True,
    )
