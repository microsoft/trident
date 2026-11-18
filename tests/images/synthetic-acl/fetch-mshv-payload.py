#!/usr/bin/env python3
"""Fetch hash-pinned official Azure Linux MSHV RPMs, preserving their licenses."""

import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import urllib.request


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", type=Path, required=True)
    args = parser.parse_args()
    for tool in ("rpm2cpio", "cpio"):
        if not shutil.which(tool):
            raise ValueError(f"Missing required tool: {tool}")
    output = args.output_dir.resolve()
    output.mkdir(parents=True, exist_ok=True)
    if any(output.iterdir()):
        raise ValueError("Payload output directory must be empty")
    rpms = output / ".rpms"
    extraction = output / ".extracted"
    rpms.mkdir()
    extraction.mkdir()
    lock = json.loads(Path(__file__).with_name("mshv-packages.json").read_text())
    for package in lock["packages"]:
        rpm = rpms / package["url"].rsplit("/", 1)[1]
        urllib.request.urlretrieve(package["url"], rpm)
        if hashlib.sha256(rpm.read_bytes()).hexdigest() != package["sha256"]:
            raise ValueError(f"RPM SHA-256 mismatch: {package['name']}")
        with subprocess.Popen(["rpm2cpio", str(rpm)], stdout=subprocess.PIPE) as producer:
            subprocess.run(["cpio", "--extract", "--make-directories", "--no-absolute-filenames"],
                           stdin=producer.stdout, cwd=extraction, check=True,
                           stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
            producer.stdout.close()
            if producer.wait() != 0:
                raise ValueError(f"RPM extraction failed: {package['name']}")
    for child in (extraction / "boot/efi").iterdir():
        shutil.move(child, output / child.name)
    shutil.move(extraction / "usr/share/licenses", output / "licenses")
    shutil.rmtree(extraction)
    (output / "payload-provenance.json").write_text(json.dumps(lock, indent=2) + "\n")


if __name__ == "__main__":
    main()
