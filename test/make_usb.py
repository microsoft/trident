#!/usr/bin/env python3
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

from build import BUILDER, GIB, HERE, IMAGE, metadata, run

LABEL = "TRD_INSTALL"
DEFAULT_SIZE_MIB = 768
PARTITION_RESERVE_MIB = 68
SECTOR_SIZE = 512


def build_usb(cosi, output, size_mib):
    """Create a new GPT/FAT32 image smaller than the COSI's target disk."""
    source = cosi.resolve(strict=True)
    info = metadata(source)
    if size_mib < 512:
        raise ValueError("A FAT32 USB test image needs at least 512 MiB")
    if size_mib * 1024**2 >= info["disk"]["size"]:
        raise ValueError("USB test media must be smaller than the COSI disk size")
    if output.exists() or output.is_symlink():
        raise FileExistsError(f"Refusing to overwrite existing USB image: {output}")

    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="usb-media-", dir=output.parent) as scratch:
        image = Path(scratch) / "usb.raw"
        with image.open("wb") as file:
            file.truncate(size_mib * 1024**2)
        volume = f"{scratch}:/media"
        base = [
            "docker",
            "run",
            "--rm",
            "-i",
            "--user",
            f"{os.getuid()}:{os.getgid()}",
            "-v",
            volume,
            BUILDER,
        ]
        subprocess.run(
            base + ["sfdisk", "/media/usb.raw"],
            input=f"label: gpt\n,{size_mib - PARTITION_RESERVE_MIB}M,U\n",
            check=True,
            text=True,
        )
        result = subprocess.run(
            base + ["sfdisk", "--json", "/media/usb.raw"],
            check=True,
            capture_output=True,
            text=True,
        )
        table = json.loads(result.stdout)["partitiontable"]
        if table["label"] != "gpt" or len(table["partitions"]) != 1:
            raise ValueError("Expected one GPT partition on USB test media")
        part = table["partitions"][0]
        device = f"{image}@@{part['start'] * SECTOR_SIZE}"
        run(["mformat", "-i", device, "-T", part["size"], "-F", "-v", LABEL, "::"])
        run(["mmd", "-i", device, "::/cosi"])
        run(["mcopy", "-i", device, source, "::/cosi/payload.cosi"])
        shutil.move(image, output)
    print(f"GPT/FAT32 installer USB: {output} ({size_mib} MiB, label {LABEL})")


def main():
    """Validate image-builder arguments and create a disposable USB fixture."""
    parser = argparse.ArgumentParser(
        description="Create a GPT/FAT32 USB image for installer VM testing"
    )
    parser.add_argument("--cosi", type=Path, default=IMAGE / "cosi/payload.cosi")
    parser.add_argument("--output", type=Path, default=HERE / ".work/installer-usb.raw")
    parser.add_argument("--size-mib", type=int, default=DEFAULT_SIZE_MIB)
    args = parser.parse_args()
    for tool in ["docker", "mformat", "mmd", "mcopy"]:
        if shutil.which(tool) is None:
            parser.error(f"{tool} is required")
    build_usb(args.cosi, args.output.resolve(), args.size_mib)


if __name__ == "__main__":
    main()
