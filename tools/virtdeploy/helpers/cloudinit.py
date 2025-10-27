from pathlib import Path
import shutil
import subprocess
import tempfile
from typing import Optional

from virtdeploy.helpers.vmmetadata import CloudInitConfig


def build_cloud_init_iso(output_path: Path, config: CloudInitConfig) -> None:

    tempdir = Path(tempfile.mkdtemp())
    try:
        shutil.copy(config.userdata, tempdir / "user-data")
        shutil.copy(config.metadata, tempdir / "meta-data")
        _create_iso(output_path, tempdir)
    finally:
        shutil.rmtree(tempdir, ignore_errors=True)


def _create_iso(output_path: Path, tempdir: Path) -> None:
    # Using the same iso options as virt-install
    # https://github.com/virt-manager/virt-manager/blob/f901c3277768a30c92daccc066b01784dccc1a05/virtinst/install/installerinject.py#L62C64-L62C80
    cmd = [
        "xorrisofs",
        "-o",
        str(output_path),
        "-J",
        "-input-charset",
        "utf8",
        "-rational-rock",
        "-V",
        "CIDATA",
        str(tempdir),
    ]

    result = subprocess.run(cmd, capture_output=True, text=True)
    if result.returncode != 0:
        raise RuntimeError(f"Failed to create cloud-init ISO: {result.stderr}")
