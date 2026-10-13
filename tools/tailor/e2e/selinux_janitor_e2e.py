#!/usr/bin/env python3
"""Reproduce Tailor's janitor denial on a disposable SELinux-enforcing Fedora VM."""

import argparse
import os
from pathlib import Path
import re
import shlex
import subprocess
import uuid


JANITOR_IMAGE = "mcr.microsoft.com/azurelinux/base/core:3.0"
WORKSPACE_NAME = "tailor-selinux-e2e-work"
IMAGE_YAML = """\
name: base-os
base:
  azureLinux:
    version: "3.0"
    variant: minimal-os
outputs:
  - format: raw
config:
  os:
    hostname: base-os
"""
TAILOR_YAML = """\
schemaVersion: 1
toolchains:
  default: ic
  entries:
    - name: ic
      container: mcr.microsoft.com/azurelinux/imagecustomizer
      tag: latest
defaults:
  outputs:
    - format: raw
"""
PRE_IC_TAILOR_YAML = """\
schemaVersion: 1
toolchains:
  default: fake
  entries:
    - name: fake
      container: mcr.microsoft.com/azurelinux/base/core
      tag: "3.0"
toolsDirSources:
  - name: base
    container: mcr.microsoft.com/azurelinux/base/core
    tag: "3.0"
runtime:
  mounts:
    dev: false
defaults:
  outputs:
    - format: raw
"""
PRE_IC_IMAGE_YAML = """\
name: preic
base:
  path: ./base.raw
toolsDir:
  source: base
outputs:
  - format: raw
config:
  previewFeatures:
    - tools-dir
  os:
    hostname: preic
"""


def run(*argv: str, cwd: Path | None = None) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(argv, cwd=cwd, capture_output=True, text=True, check=False)
    print(f"$ {' '.join(argv)}\nexit={result.returncode}")
    if result.stdout.strip():
        print(result.stdout.strip())
    if result.stderr.strip():
        print(result.stderr.strip())
    return result


def require(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def label(path: Path) -> bytes:
    return os.getxattr(path, "security.selinux")


def under_prefix(value: str, prefix: Path) -> bool:
    value = value.removesuffix(" (deleted)")
    if not value.startswith("/"):
        return False
    return Path(os.path.normpath(value)).is_relative_to(prefix)


def unescape_mount_path(value: str) -> str:
    encoded = value.encode("utf-8")
    decoded = bytearray()
    index = 0
    while index < len(encoded):
        if encoded[index] != ord("\\"):
            decoded.append(encoded[index])
            index += 1
            continue
        digits = encoded[index + 1:index + 4]
        if len(digits) != 3 or any(digit not in b"01234567" for digit in digits):
            raise RuntimeError(f"refusing malformed mountinfo escape: {value}")
        number = int(digits, 8)
        if number > 255:
            raise RuntimeError(f"refusing malformed mountinfo escape: {value}")
        decoded.append(number)
        index += 4
    try:
        return decoded.decode("utf-8")
    except UnicodeDecodeError as error:
        raise RuntimeError(f"refusing malformed mountinfo encoding: {value}") from error


def assert_no_live_references(
    target: Path, loop_backings: list[tuple[str, str]], mountinfo: str,
) -> None:
    for device, backing in loop_backings:
        if under_prefix(backing, target):
            raise RuntimeError(f"refusing to delete live {device} backing file: {backing}")

    for line in mountinfo.splitlines():
        before, separator, after = line.partition(" - ")
        fields, filesystem = before.split(), after.split()
        if not separator or len(fields) < 5 or len(filesystem) < 2:
            raise RuntimeError(f"refusing deletion with malformed mountinfo: {line}")
        for field in (fields[3], fields[4], filesystem[1]):
            value = unescape_mount_path(field)
            if under_prefix(value, target):
                raise RuntimeError(f"refusing to delete live mount reference: {value}")


def assert_no_local_live_references(
    target: Path, sys_block: Path = Path("/sys/block"),
    mountinfo: Path = Path("/proc/self/mountinfo"),
) -> None:
    # The VM fixture uses a local daemon; this client-namespace scan is not a remote-engine barrier.
    require(target.is_absolute() and target.exists() and target == target.resolve(strict=True),
            f"refusing missing or symlinked deletion target: {target}")
    loop_backings = []
    for block in sys_block.iterdir():
        if not re.fullmatch(r"loop\d+", block.name):
            continue
        try:
            (block / "loop").stat()
        except FileNotFoundError:
            continue
        loop_backings.append((block.name, (block / "loop/backing_file").read_text().strip()))
    mount_data = mountinfo.read_text()
    require(bool(mount_data.strip()), "refusing deletion with empty mountinfo")
    assert_no_live_references(target, loop_backings, mount_data)


def probe_stale_tools_dir(
    tailor: str, podman: list[str], endpoint: str, root: Path, expect_fixed: bool,
) -> None:
    workspace = root / f"pre-ic-{uuid.uuid4().hex}"
    image_dir = workspace / "preic"
    image_dir.mkdir(parents=True)
    (workspace / "tailor.yaml").write_text(PRE_IC_TAILOR_YAML)
    (image_dir / "image.yaml").write_text(PRE_IC_IMAGE_YAML)
    (image_dir / "base.raw").write_bytes(b"not an image: fake IC must never read this")
    matrix = run(tailor, "matrix", "--format", "slugs", cwd=workspace)
    slugs = matrix.stdout.strip().splitlines()
    require(matrix.returncode == 0 and len(slugs) == 1 and re.fullmatch(r"[a-zA-Z0-9_.-]+", slugs[0]),
            f"expected one safe cell slug, got {slugs!r}")
    dry = run(tailor, "build", "preic", "--dry-run", cwd=workspace)
    bind_sources = re.findall(r"(?m)^\s+-v (\S+?):", dry.stdout)
    require(dry.returncode == 0 and bind_sources
            and all(Path(source).is_relative_to(workspace) for source in bind_sources),
            f"fake IC must bind only its disposable guest workspace, got {bind_sources!r}")

    parent = workspace / "artifacts/.tailor/build" / slugs[0]
    parent.mkdir(parents=True)
    stale = parent / "tools-dir"
    marker = stale / "stale-marker"
    bind = f"{parent}:{parent}"
    seeded = run(
        *podman, "run", "--rm", "--security-opt", "label=disable",
        "-v", bind, JANITOR_IMAGE, "/bin/sh", "-c",
        f"mkdir -p -- {shlex.quote(str(stale))} && printf stale > {shlex.quote(str(marker))}",
    )
    require(seeded.returncode == 0 and marker.stat().st_uid == 0,
            "could not seed disposable root-owned stale tools-dir marker")
    parent_label = label(parent)
    require(b":user_home_t:" in label(stale) and b":user_home_t:" in parent_label,
            "pre-IC fixture must retain its user_home_t labels")
    assert_no_local_live_references(stale)

    result = run(tailor, "--engine", "podman", "--host",
                 endpoint, "build", "preic", cwd=workspace)
    output = result.stdout + result.stderr
    require(result.returncode != 0, "fake IC must fail after the pre-IC janitor phase")
    require(label(parent) == parent_label, "build attempt changed the guarded parent label")
    if expect_fixed:
        require(not marker.exists(), "stale tools-dir marker survived: pre-IC cleanup not proven")
        require("Permission denied" not in output and "janitor exited" not in output,
                "build failed in janitor rather than the intentionally fake IC toolchain")
        require("customize" in output and "not found" in output,
                "build did not reach the deliberately missing fake IC command")
        print("PASS: non-root Tailor passed stale tools-dir pre-clean; fake IC failed later")
    else:
        require(marker.exists() and "Permission denied" in output and "janitor exited" in output,
                "baseline did not fail at the root-owned stale tools-dir pre-clean")
        print("PASS: baseline Tailor denied at stale tools-dir pre-clean")
    print("NOTE: fake IC never produces an image; post-build chown is not exercised")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tailor-bin", type=Path, required=True)
    parser.add_argument("--podman-socket", default="/run/podman/podman.sock")
    parser.add_argument("--expect-fixed", action="store_true",
                        help="require janitor cleanup to pass (the fake IC still fails in pre-IC mode)")
    parser.add_argument("--probe-stale-tools-dir", action="store_true",
                        help="probe real pre-IC janitor removal using a deliberately failing toolchain")
    args = parser.parse_args()

    require(os.geteuid() != 0, "run this test as an ordinary Fedora user, not root")
    require(run("getenforce").stdout.strip() == "Enforcing", "SELinux must be enforcing")
    info = run(
        "podman", "--remote", "--url", f"unix://{args.podman_socket}",
        "info", "--format", "{{.Host.Security.SELinuxEnabled}} {{.Host.Security.Rootless}}",
    )
    require(info.returncode == 0 and info.stdout.strip() == "true false",
            "the non-root test user must access a rootful, SELinux-enabled Podman API")

    root = Path.home() / WORKSPACE_NAME
    require(not root.is_symlink(), f"refusing symlinked fixture root: {root}")
    root.mkdir(exist_ok=True)
    endpoint = f"unix://{args.podman_socket}"
    podman = ["podman", "--remote", "--url", endpoint]
    tailor = str(args.tailor_bin.resolve(strict=True))
    if args.probe_stale_tools_dir:
        probe_stale_tools_dir(tailor, podman, endpoint, root, args.expect_fixed)
        return

    workspace = root / f"run-{uuid.uuid4().hex}"
    artifacts = workspace / "artifacts"
    image_dir = workspace / "base-os"
    artifacts.mkdir(parents=True)
    image_dir.mkdir()
    (workspace / "tailor.yaml").write_text(TAILOR_YAML)
    (image_dir / "image.yaml").write_text(IMAGE_YAML)
    artifact = artifacts / "base-os_amd64_raw.raw"
    parent_label = label(artifacts)
    require(b":user_home_t:" in parent_label,
            f"expected user_home_t for positive control; got {parent_label!r}")

    bind = f"{artifacts}:{artifacts}"
    # The rootful container stands in for IC's root-owned output; no sudo or host writes.
    created = run(
        *podman, "run", "--rm", "--security-opt", "label=disable",
        "-v", bind, JANITOR_IMAGE, "/bin/sh", "-c",
        f"printf fixture > '{artifact}'",
    )
    require(created.returncode == 0, "could not create disposable root-owned IC-like output")
    require(artifact.stat().st_uid == 0, "fixture artifact is not root-owned")
    original_label = label(artifact)
    require(b":user_home_t:" in original_label, f"unexpected artifact label: {original_label!r}")
    print(f"positive control: artifact={artifact}, label={original_label!r}")

    container = run(*podman, "run", "--rm", JANITOR_IMAGE, "/bin/cat",
                    "/proc/self/attr/current")
    require(container.returncode == 0 and "container_t:" in container.stdout,
            "janitor container did not run confined as container_t")

    assert_no_local_live_references(artifact)
    baseline = run(*podman, "run", "--rm", "-v", bind, JANITOR_IMAGE,
                   "/bin/rm", "-rf", "--", str(artifact))
    require(baseline.returncode != 0 and "Permission denied" in baseline.stderr
            and artifact.exists(), "unmodified janitor bind must be denied by SELinux")
    print("PASS: actual container_t janitor rm cannot remove a user_home_t artifact")

    assert_no_local_live_references(artifact)
    actual = run(tailor, "--engine", "podman", "--host",
                 f"unix://{args.podman_socket}", "clean", "base-os", cwd=workspace)
    if args.expect_fixed:
        require(actual.returncode == 0 and not artifact.exists(),
                "Tailor clean still fails against user_home_t with SELinux enforcing")
        print("PASS: actual non-root Tailor janitor removed the root-owned artifact")
        recreated = run(
            *podman, "run", "--rm", "--security-opt", "label=disable",
            "-v", bind, JANITOR_IMAGE, "/bin/sh", "-c", f"printf fixture > '{artifact}'",
        )
        require(recreated.returncode == 0, "could not recreate artifact for workaround control")
    else:
        require(actual.returncode != 0 and "Permission denied" in actual.stderr + actual.stdout,
                "Tailor clean did not fail with the expected SELinux permission denial")
        require(artifact.exists() and artifact.stat().st_uid == 0,
                "failed janitor unexpectedly removed or changed the positive-control artifact")
        print("PASS: actual non-root Tailor clean fails against user_home_t with SELinux enforcing")

    # Demonstrate a container-scoped workaround without changing policy or relabeling the host.
    assert_no_local_live_references(artifact)
    fixed = run(*podman, "run", "--rm", "--security-opt", "label=disable",
                "-v", bind, JANITOR_IMAGE, "/bin/rm", "-rf", "--", str(artifact))
    require(fixed.returncode == 0 and not artifact.exists(),
            "container-scoped label=disable did not allow the same janitor removal")
    require(label(artifacts) == parent_label,
            "workaround must not relabel the disposable bind source")
    print("PASS: same janitor rm succeeds with per-container label=disable; host policy/labels unchanged")
    if args.expect_fixed:
        print("PASS: patched Tailor janitor succeeded; no real Image Customizer build was run")
    else:
        print("NOTE: baseline Tailor is not fixed; no real Image Customizer build was run")


if __name__ == "__main__":
    main()
