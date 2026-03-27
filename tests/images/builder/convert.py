import logging
from pathlib import Path
import subprocess
import sys
from typing import Optional

from builder import utils

logging.basicConfig(level=logging.DEBUG)
log = logging.getLogger(__name__ if __name__ != "__main__" else "convert-image")


def convert_image(
    container_image: str,
    config_name: str,
    base_image: Path,
    img_format: str,
    output_file: Path,
    image_architecture: Optional[str] = None,
    dry_run: bool = False,
):
    """
    Convert an image to a `baremetal-image` using AZL Image Customizer `convert` via Docker container.

    Args:
        container_image: Docker container image for Image Customizer
        config_name: Name of Image Customizer config
        base_image: Path to the base image file to customize
        img_format: Output image format
        output_file: Path where the customized image will be saved
        dry_run: If True, only log the command without executing it

    Raises:
        Exception: If the Image Customizer container execution fails
    """
    log.info(f"Building '{config_name}'")

    base_cmd = [
        "docker",
        "run",
        "--rm",
        "--privileged",
        "-v",
        f"/:{utils.HOST_PATH}",
        "-v",
        "/dev:/dev",
    ]

    if image_architecture:
        base_cmd.append("--platform")
        base_cmd.append(image_architecture)

    base_cmd.extend(
        [
            container_image,
            "convert",
            "--log-level",
            "debug",
            "--build-dir",
            utils.BUILD_DIR,
            "--image-file",
            utils.build_path(base_image),
            "--output-image-format",
            img_format,
            "--output-image-file",
            utils.build_path(output_file),
        ]
    )

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
