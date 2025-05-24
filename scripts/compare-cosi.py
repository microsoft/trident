#!/usr/bin/env python3

"""
A script to thoroughly compare two COSI files.
It extracts the contents of the COSI files, compares the files and directories,
and generates a report of the differences.

If it finds a UKI file in both with different content, it will extract the initrd
and compare the contents of the initrd files.

It optionally outputs the diff trees to a specified directory for further analysis.

Requires:
- Python 3.12 (I think) or higher
- pefile

Usage:
    python3 compare-cosi.py <cosi_file_a> <cosi_file_b> [-o <output_dir>]
"""

import argparse
from contextlib import contextmanager
from dataclasses import dataclass
import gzip
import hashlib
import logging
import json
import shutil
from pathlib import Path
import tarfile
import tempfile
from typing import Dict, Generator, List, Optional, Tuple
import subprocess
from io import StringIO

# Set up logging
logging.basicConfig(level=logging.DEBUG)
log = logging.getLogger("compare-cosi")

try:
    import pefile
except ImportError:
    log.critical(
        "pefile is not installed. Please install it using 'pip install pefile'."
    )
    exit(1)


def parse_args():
    parser = argparse.ArgumentParser(
        description="Compare two COSI files and optionally write results to a file."
    )
    parser.add_argument("cosi_file_a", type=Path, help="Path to the first COSI file.")
    parser.add_argument("cosi_file_b", type=Path, help="Path to the second COSI file.")
    parser.add_argument(
        "-o",
        "--output",
        type=Path,
        default=None,
        help="Optional path to write the comparison results.",
    )
    return parser.parse_args()


@contextmanager
def cosi_extractor(cosi_file_path: Path) -> "Generator[Path, None, None]":
    """
    Extracts the given COSI file (assumed to be a zip archive) to the specified mount path.
    If the mount path does not exist, it will be created.
    """

    with tempfile.TemporaryDirectory(prefix=f"{cosi_file_path.name}-") as work_dir:
        work_path = Path(work_dir)
        log.debug(f"Temporary work directory for '{cosi_file_path}': '{work_path}'")

        mounted = []

        try:
            yield extract_and_mount_cosi(mounted, cosi_file_path, work_path)
        finally:
            for mount_point in reversed(mounted):
                log.debug(f"Unmounting {mount_point}")
                # Unmount the image
                unmount_command = ["umount", str(mount_point)]
                subprocess.run(unmount_command, check=True)


def extract_and_mount_cosi(
    mounted: List[Path],
    cosi_file_path: Path,
    work_path: Path,
) -> Path:
    if not work_path.exists():
        raise FileNotFoundError(f"Work path does not exist: {work_path}")

    extract_path = work_path / "extracted"
    decompression_path = work_path / "decompressed"

    with open(cosi_file_path, "rb") as cosi_file:
        with tarfile.open(fileobj=cosi_file, mode="r:*") as tar:
            tar.extractall(path=extract_path, filter="data")

    metadata_path = extract_path / "metadata.json"
    if not metadata_path.exists():
        raise FileNotFoundError(f"Metadata file does not exist: {metadata_path}")
    log.info(f"Extracted COSI file to: {extract_path}")

    with open(metadata_path, "r") as metadata_file:
        metadata = json.load(metadata_file)

    root_path = work_path / "root"
    root_path.mkdir(parents=True, exist_ok=True)

    # Mount all images
    image_mount: List[Tuple[Path, Path]] = [
        (Path(image["mountPoint"]), Path(image["image"]["path"]))
        for image in metadata["images"]
    ]

    image_mount.sort(key=lambda x: len(x[0].parts))

    for mount_point, image_path in image_mount:
        effective_mount_point = root_path / mount_point.relative_to("/")

        # VERY IMPORTANT SAFETY CHECKS TO AVOID WEIRD ISSUES!
        assert effective_mount_point.is_absolute()
        # Python 3.8 compatibility: check if effective_mount_point is under root_path
        try:
            effective_mount_point.relative_to(root_path)
        except ValueError:
            raise AssertionError("Mount point must be inside root_path")

        mount_point.mkdir(parents=True, exist_ok=True)
        image_path = extract_path / image_path
        if not image_path.exists():
            raise FileNotFoundError(f"Image path does not exist: {image_path}")

        # Decompress the image file using zstd
        decompressed_image_path = decompression_path / image_path.relative_to(
            extract_path
        ).with_suffix(".raw")

        decompressed_image_path.parent.mkdir(parents=True, exist_ok=True)
        subprocess.run(
            ["zstd", "-d", "-f", str(image_path), "-o", str(decompressed_image_path)],
            check=True,
        )

        log.debug(f"Mounting {decompressed_image_path} to {effective_mount_point}")

        if not effective_mount_point.exists():
            effective_mount_point.mkdir(parents=True, exist_ok=True)

        # Mount the decompressed image
        mount_command = [
            "mount",
            "-o",
            "loop",
            str(decompressed_image_path),
            str(effective_mount_point),
        ]
        log.debug(f"Mounting {decompressed_image_path} to {effective_mount_point}")
        subprocess.run(mount_command, check=True)
        mounted.append(effective_mount_point)

    return root_path


def hash_file(path: Path) -> str:
    """Return SHA256 hash of file contents."""
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(8192), b""):
            h.update(chunk)
    return h.hexdigest()


class TreeCompare:
    def __init__(self, tree_a: Path, tree_b: Path):
        self.tree_a = tree_a
        self.tree_b = tree_b
        self.same_files: List[Path] = []
        self.only_in_tree_a: List[Path] = []
        self.only_in_tree_b: List[Path] = []
        self.diff_files: List[Path] = []

        for root, _, files in tree_a.walk():
            rel_root = Path(root).relative_to(tree_a)
            for file in files:
                rel_path = rel_root / file
                path_a = tree_a / rel_path
                path_b = tree_b / rel_path

                if path_b.exists(follow_symlinks=False):
                    if path_b.is_file():
                        if hash_file(path_a) == hash_file(path_b):
                            self.same_files.append(rel_path)
                        else:
                            self.diff_files.append(rel_path)
                elif path_a.is_file():
                    self.only_in_tree_a.append(rel_path)
                else:
                    # This is a directory that only exists in tree A.
                    # We don't have to do anything as we only care about files.
                    pass

        for root, _, files in tree_b.walk():
            rel_root = Path(root).relative_to(tree_b)
            for file in files:
                rel_path = rel_root / file
                path_a = tree_a / rel_path
                if not path_a.exists(follow_symlinks=False):
                    self.only_in_tree_b.append(rel_path)

    def diftree_a(self) -> List[Path]:
        return [file for file in self.diff_files + self.only_in_tree_a]

    def diftree_b(self) -> List[Path]:
        return [file for file in self.diff_files + self.only_in_tree_b]

    def __copy_diftree(self, base: Path, files: List[Path], dest: Path):
        for file in files:
            source = base / file
            dest_file = dest / file
            dest_file.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy(source, dest_file, follow_symlinks=False)

    def copy_diftree_a(self, dest: Path):
        """Copy files only in tree A and files with different content to the destination."""
        log.info(f"Copying diftree A to '{dest}' from '{self.tree_a}'")
        self.__copy_diftree(self.tree_a, self.diftree_a(), dest)

    def copy_diftree_b(self, dest: Path):
        """Copy files only in tree B and files with different content to the destination."""
        log.info(f"Copying diftree B to '{dest}' from '{self.tree_b}'")
        self.__copy_diftree(self.tree_b, self.diftree_b(), dest)

    def copy_diftrees(self, a_dest: Path, b_dest: Path):
        """Copy files only in tree A and B and files with different content to the respective destinations."""
        a_dest.mkdir(parents=True, exist_ok=True)
        b_dest.mkdir(parents=True, exist_ok=True)

        self.copy_diftree_a(a_dest)
        self.copy_diftree_b(b_dest)
        log.info(f"Copied differing files to:\n - A: {a_dest}/\n - B: {b_dest}/")

    def report(self, a_name: str = None, b_name: str = None) -> str:
        """Generate a report of the differences."""
        if a_name is None:
            a_name = str(self.tree_a)
        if b_name is None:
            b_name = str(self.tree_b)

        report = StringIO()
        report.write(f"Files only in '{a_name}':\n")
        for file in self.only_in_tree_a:
            report.write(f"  {file}\n")
        report.write(f"Files only in '{b_name}':\n")
        for file in self.only_in_tree_b:
            report.write(f"  {file}\n")
        report.write(f"Files with different content:\n")
        for file in self.diff_files:
            report.write(f"  {file}\n")
        return report.getvalue()


@dataclass
class UkiData:
    path: Path
    sections: List[str]
    initrd: Path


@contextmanager
def uki_extractor(uki_path: Path) -> Generator[UkiData, None, None]:
    with tempfile.TemporaryDirectory() as work_dir:
        work_path = Path(work_dir)
        yield extract_uki(uki_path, work_path)
        # Cleanup is handled by the context manager


def extract_uki_section(
    sections: Dict[str, pefile.SectionStructure], section_name: str, target: Path
):
    """
    Extract a specific section from the PE file.
    """
    if section_name not in sections:
        log.error(f"Section {section_name} not found in PE file.")
        raise ValueError(f"Section {section_name} not found in PE file.")

    section = sections[section_name]
    log.info(f"Extracting section '{section_name}' to '{target}'")
    with open(target, "wb") as f:
        f.write(section.get_data())
    return target


def detect_compression(path: Path) -> str:
    with open(path, "rb") as f:
        magic = f.read(4)
    if magic.startswith(b"\x1f\x8b"):
        return "gzip"
    elif magic == b"\x28\xb5\x2f\xfd":
        return "zstd"
    else:
        return "unknown"


def extract_initrd(path: Path, target: Path):
    """
    Extract the initrd file from the given path.
    """
    compression = detect_compression(path)
    log.info(f"Detected compression: {compression}")

    target.mkdir(parents=True, exist_ok=True)

    with tempfile.TemporaryDirectory() as work_dir:
        work_path = Path(work_dir)
        staging_file = work_path / "staging_file"
        if compression == "gzip":
            with gzip.open(path, "rb") as gzf:
                with open(staging_file, "wb") as out_f:
                    shutil.copyfileobj(gzf, out_f)
        elif compression == "zstd":
            subprocess.run(
                ["zstd", "-d", str(path), "-o", str(staging_file)], check=True
            )
        else:
            raise ValueError(f"Unsupported compression type: {compression}")

        # Extract the cpio archive from staging_file into target
        subprocess.run(
            ["cpio", "-idmv"],
            cwd=target,
            input=staging_file.read_bytes(),
            check=True,
            capture_output=True,
        )


def extract_uki(uki_path: Path, workdir: Path) -> UkiData:
    log.info(f"Extracting UKI file: {uki_path}")
    with open(uki_path, "rb") as f:
        data = f.read()
    pe = pefile.PE(data=data)

    available_sections = {}
    section: pefile.SectionStructure
    for section in pe.sections:
        name: str = section.Name.decode("utf-8").strip("\x00")
        available_sections[name] = section
    log.debug(f"Sections in UKI file: {','.join(available_sections.keys())}")

    # Extract all sections
    initrd_img = workdir / "initrd.img"
    extract_uki_section(available_sections, ".initrd", initrd_img)
    initrd_extracted = workdir / "initrd"
    extract_initrd(initrd_img, workdir / "initrd")

    pe.close()
    return UkiData(
        path=uki_path,
        sections=list(available_sections.keys()),
        initrd=initrd_extracted,
    )


def compare_ukis(
    *,
    uki_path: Path,
    cosi_a_path: Path,
    cosi_a_root: Path,
    cosi_b_path: Path,
    cosi_b_root: Path,
    output: Optional[Path] = None,
):
    """
    Compare the UKI files in the two COSI files.
    This is a placeholder function and should be implemented based on your requirements.
    """
    with uki_extractor(cosi_a_root / uki_path) as uki_a, uki_extractor(
        cosi_b_root / uki_path
    ) as uki_b:
        # Compare the two UKI files
        log.info(f"Comparing UKI files: {uki_a.path} and {uki_b.path}")
        # Implement your comparison logic here
        initrd_compare = TreeCompare(uki_a.initrd, uki_b.initrd)

        report = initrd_compare.report(a_name=cosi_a_path.name, b_name=cosi_b_path.name)
        print(report)

        if output:
            initrd_compare.copy_diftrees(
                output / f"{cosi_a_path.name}-initrd",
                output / f"{cosi_b_path.name}-initrd",
            )

            with open(output / "initrd-report.txt", "w") as report_file:
                report_file.write(report)


def main():
    args = parse_args()
    cosi_file_a: Path = args.cosi_file_a
    cosi_file_b: Path = args.cosi_file_b
    output: Optional[Path] = args.output

    if not cosi_file_a.exists():
        log.error(f"COSI file 1 does not exist: {cosi_file_a}")
        exit(1)

    if not cosi_file_b.exists():
        log.error(f"COSI file 2 does not exist: {cosi_file_b}")
        exit(1)

    if output:
        if output.exists():
            shutil.rmtree(output)
        output.mkdir(parents=True, exist_ok=True)
        log.info(f"Results will be written to: {output}")
    else:
        log.info("No output file specified.")

    log.info(f"COSI file 1: {cosi_file_a}")
    log.info(f"COSI file 2: {cosi_file_b}")

    with tempfile.TemporaryDirectory() as work_dir:
        log.info(f"Temporary work directory: {work_dir}")
        work_dir = Path(work_dir)
        cosi_a_workdir = work_dir / "cosi_a"
        cosi_b_workdir = work_dir / "cosi_b"

        cosi_a_workdir.mkdir(parents=True, exist_ok=True)
        cosi_b_workdir.mkdir(parents=True, exist_ok=True)

        with cosi_extractor(cosi_file_a) as cosi_a_root, cosi_extractor(
            cosi_file_b
        ) as cosi_b_root:
            # Compare the two COSI files
            log.info(f"Comparing {cosi_file_a} and {cosi_file_b}")
            log.debug(f"COSI A root: {cosi_a_root}")
            log.debug(f"COSI B root: {cosi_b_root}")

            # Compare the two directories
            compare = TreeCompare(cosi_a_root, cosi_b_root)

            report = compare.report()
            print(report)

            if output:
                with open(output / "report.txt", "w") as report_file:
                    report_file.write(report)

                compare.copy_diftrees(
                    output / cosi_file_a.name,
                    output / cosi_file_b.name,
                )

            for file in compare.diff_files:
                if file.suffix == ".efi":
                    log.info(f"UKI file found: {file}")
                    compare_ukis(
                        uki_path=file,
                        cosi_a_path=cosi_file_a,
                        cosi_a_root=cosi_a_root,
                        cosi_b_path=cosi_file_b,
                        cosi_b_root=cosi_b_root,
                        output=args.output,
                    )


if __name__ == "__main__":
    main()
