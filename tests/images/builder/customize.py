import logging
from pathlib import Path
import subprocess
import sys
from typing import List

logging.basicConfig(level=logging.DEBUG)
log = logging.getLogger(__name__ if __name__ != "__main__" else "customize-image")

BUILD_DIR = "/tmp"
HOST_PATH = Path("/host")


def build_config(
    container_image: str,
    config_name: str,
    yaml_path: Path,
    base_image: Path,
    img_format: str,
    output_file: Path,
    rpm_sources: List[Path] = [],
    dry_run: bool = False,
):
    """
    Build a custom image using AZL Image Customizer via Docker container.

    Note: Image Customizer no longer supports running as a raw binary and must be
    executed within a Docker container. This function orchestrates the containerized
    build process.

    Args:
        container_image: Docker container image for Image Customizer
        config_name: Name of Image Customizer config
        yaml_path: Path to the Image Customizer YAML configuration file
        base_image: Path to the base image file to customize
        img_format: Output image format
        output_file: Path where the customized image will be saved
        rpm_sources: List of paths to additional RPM source directories
        dry_run: If True, only log the command without executing it

    Raises:
        Exception: If the Image Customizer container execution fails
    """
    log.info(f"Building '{config_name}'")
    log.info(f"Using YAML: {yaml_path}")

    base_cmd = [
        "docker",
        "run",
        "--rm",
        "--privileged",
        "-v",
        f"/:{HOST_PATH}",
        "-v",
        "/dev:/dev",
        container_image,
        "--config-file",
        build_path(yaml_path),
        "--log-level",
        "debug",
        "--build-dir",
        BUILD_DIR,
        "--image-file",
        build_path(base_image),
        "--output-image-format",
        img_format,
        "--output-image-file",
        build_path(output_file),
    ]

    for _, rpm in enumerate(rpm_sources):
        base_cmd.append("--rpm-source")
        base_cmd.append(build_path(rpm))

    # Stringify all the args
    base_cmd = [str(x) for x in base_cmd]

    cmd = " \\\n    ".join(base_cmd)

    log.debug(f"Running:\n  {cmd}")

    if dry_run:
        log.info("Dry run, not executing command")
        return

    # Run the command
    try:
        result = subprocess.run(base_cmd, stdout=sys.stdout, stderr=sys.stderr)
        result.check_returncode()
    except Exception as e:
        log.error(f"Error building config '{config_name}': {e}")
        raise e


def build_path(path: Path) -> Path:
    """
    Convert a host filesystem path to its corresponding path inside the Docker container.

    The Docker container mounts the host's root filesystem at /host, so this function
    transforms absolute host paths like '/home/user/file.txt' to container paths
    like '/host/home/user/file.txt'.

    Args:
        path: Absolute or relative path on the host filesystem

    Returns:
        Path that can be used inside the Docker container to access the same file

    Example:
        build_path(Path("/home/user/config.yaml")) -> Path("/host/home/user/config.yaml")
    """
    return HOST_PATH / path.absolute().relative_to(Path("/"))


def inject_files(
    container_image: str,
    inject_files_yaml_path: Path,
    unsigned_image_file: Path,
    img_format: str,
    output_image_file: Path,
    dry_run: bool = False,
):
    """
    Run the imagecustomizer inject-files command to inject files into the image.

    Args:
        container_image: Image Customizer container image to run the command in
        inject_files_yaml_path: Path to the inject-files YAML configuration file listing the signed
        and unsigned sources
        unsigned_image_file: Path to the unsigned image
        img_format: Format of the output image
        output_image_file: Path to the signed output image
        dry_run: If True, do not run the command

    Raises:
        Exception: If docker command fails.
    """
    base_cmd = [
        "docker",
        "run",
        "--rm",
        "--privileged",
        "-v",
        f"/:{HOST_PATH}",
        "-v",
        "/dev:/dev",
        container_image,
        "inject-files",
        "--config-file",
        build_path(inject_files_yaml_path),
        "--log-level",
        "debug",
        "--build-dir",
        BUILD_DIR,
        "--image-file",
        build_path(unsigned_image_file),
        "--output-image-format",
        img_format,
        "--output-image-file",
        build_path(output_image_file),
    ]

    # Stringify all the args
    base_cmd = [str(x) for x in base_cmd]

    cmd = " \\\n    ".join(base_cmd)

    log.debug(f"Running:\n  {cmd}")

    if dry_run:
        log.info("Dry run, not executing command")
        return

    # Run the command
    try:
        result = subprocess.run(base_cmd, stdout=sys.stdout, stderr=sys.stderr)
        result.check_returncode()
    except Exception as e:
        log.error(f"Error running inject-files using YAML: {inject_files_yaml_path}")
        raise e
