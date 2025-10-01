import logging
import os
import shutil
import subprocess
import tempfile
from typing import Optional

from contextlib import contextmanager
from pathlib import Path


@contextmanager
def temp_dir(
    prefix: Optional[str] = None, dir: Optional[Path] = None, sudo: bool = False
):
    """
    Context manager for temporary directory cleanup.

    Args:
        prefix: Prefix for the temp directory name
        dir: Parent directory in which to create the temp dir (default None, system temp dir)
        sudo: Whether to use sudo for cleanup

    Yields:
        Path: Path of temp dir
    """
    parent = str(dir) if dir is not None else None
    build_dir = tempfile.mkdtemp(prefix=prefix, dir=parent)
    try:
        yield Path(build_dir)
    finally:
        logging.debug(f"Cleaning up build dir: {build_dir}")
        if sudo:
            logging.debug(f"Removing build dir as root: {build_dir}")
            subprocess.run(["sudo", "rm", "-rf", build_dir], check=True)
        else:
            shutil.rmtree(build_dir)


@contextmanager
def temp_file(path: Path, sudo: bool = False):
    """
    Context manager that deletes the specified file upon exit.

    Args:
        path: Path of the temporary file to remove
        sudo: Whether to use sudo for cleanup

    Yields:
        Path: Path of the temp file (optional, you can just yield)
    """
    try:
        yield path
    finally:
        if path.exists():
            logging.debug(f"Cleaning up temp file: {path}")
            if sudo:
                subprocess.run(["sudo", "rm", "-f", str(path)], check=True)
            else:
                os.remove(path)
