#!/usr/bin/env python3

"""Generate Go protobuf and gRPC stubs for trident.v1 and trident.v1preview.

This script discovers all .proto files under proto/trident/{v1,v1preview},
computes the --go_opt=M mappings so each file is routed to the correct Go
package, and invokes protoc once per package.

Usage (called by go:generate in grpc.go):
    ./generate.py
"""

import glob
import os
import shutil
import subprocess
import sys
from pathlib import Path

# Resolve paths relative to this script's location.
SCRIPT_DIR = Path(__file__).resolve().parent
PROTO_ROOT = SCRIPT_DIR / ".." / ".." / ".." / "proto"
PROTO_ROOT = PROTO_ROOT.resolve()

GO_MODULE = "tridenttools/pkg/tridentgrpc"

# Map each proto subdirectory to its Go output package name.
PACKAGES = {
    "trident/v1": "tridentpbv1",
    "trident/v1preview": "tridentpbv1preview",
}


def discover_proto_files(proto_dir: str) -> list[str]:
    """Return all .proto files under proto_dir, as paths relative to PROTO_ROOT."""
    abs_dir = PROTO_ROOT / proto_dir
    files = sorted(glob.glob(str(abs_dir / "*.proto")))
    return [os.path.relpath(f, PROTO_ROOT) for f in files]


def build_m_flags(all_files: dict[str, list[str]]) -> list[str]:
    """Build --go_opt=M and --go-grpc_opt=M flags for every discovered proto file.

    Each file gets mapped to its package's Go import path so that cross-package
    imports (e.g. v1preview importing v1 types) resolve correctly.
    """
    go_opt_flags: list[str] = []
    grpc_opt_flags: list[str] = []

    for proto_dir, files in all_files.items():
        go_pkg = PACKAGES[proto_dir]
        go_import_path = f"{GO_MODULE}/{go_pkg}"
        for f in files:
            go_opt_flags.append(f"--go_opt=M{f}={go_import_path}")
            grpc_opt_flags.append(f"--go-grpc_opt=M{f}={go_import_path}")

    return go_opt_flags + grpc_opt_flags


def compile_package(
    proto_dir: str,
    go_pkg: str,
    proto_files: list[str],
    m_flags: list[str],
) -> None:
    """Run protoc for a single Go package."""
    out_dir = SCRIPT_DIR / go_pkg
    out_dir.mkdir(exist_ok=True)

    cmd = [
        "protoc",
        f"-I{PROTO_ROOT}",
        f"--go_out={out_dir}",
        "--go_opt=paths=source_relative",
        f"--go-grpc_out={out_dir}",
        "--go-grpc_opt=paths=source_relative",
        *m_flags,
        *[str(PROTO_ROOT / f) for f in proto_files],
    ]

    print(f"  protoc {proto_dir} -> {go_pkg}/ ({len(proto_files)} files)")
    result = subprocess.run(cmd, capture_output=True, text=True)
    if result.returncode != 0:
        print(f"protoc failed for {proto_dir}:", file=sys.stderr)
        print(result.stderr, file=sys.stderr)
        sys.exit(result.returncode)

    # protoc with paths=source_relative mirrors the proto directory structure
    # under out_dir (e.g. tridentv1/trident/v1/*.pb.go). Move the generated
    # files up to out_dir and remove the empty nesting.
    nested_dir = out_dir / proto_dir
    if nested_dir.is_dir():
        for pb_file in nested_dir.glob("*.go"):
            shutil.move(str(pb_file), str(out_dir / pb_file.name))
        # Remove the now-empty nested directories.
        shutil.rmtree(out_dir / proto_dir.split("/")[0])


def main() -> None:
    print(f"Proto root: {PROTO_ROOT}")

    # Discover all proto files per package.
    all_files: dict[str, list[str]] = {}
    for proto_dir in PACKAGES:
        files = discover_proto_files(proto_dir)
        if not files:
            print(
                f"Warning: no .proto files found in {PROTO_ROOT / proto_dir}",
                file=sys.stderr,
            )
            continue
        all_files[proto_dir] = files

    # Build the M-flags covering every file across all packages so cross-package
    # imports resolve correctly.
    m_flags = build_m_flags(all_files)

    # Compile each package.
    for proto_dir, files in all_files.items():
        compile_package(proto_dir, PACKAGES[proto_dir], files, m_flags)

    print("Done.")


if __name__ == "__main__":
    main()
