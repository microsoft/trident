#!/usr/bin/env python3
import argparse
import datetime
import json
from pathlib import Path
import shlex
import shutil
import subprocess
import tarfile
import tempfile
import tomllib

HERE = Path(__file__).resolve().parent
REPO = HERE.parent
IMAGE = HERE / "installer"
WORK = HERE / ".work"
BUILDER = "azl3/trident-builder:latest"
GIB = 1024**3


def run(command, **kwargs):
    print("+", shlex.join(map(str, command)), flush=True)
    subprocess.run(list(map(str, command)), check=True, **kwargs)


def metadata(path):
    with tarfile.open(path, "r:") as archive:
        member = archive.getmember("metadata.json")
        if not member.isfile() or member.size > 16 * 1024**2:
            raise ValueError("COSI metadata is not a bounded regular file")
        info = json.load(archive.extractfile(member))
    release = info.get("osRelease", "")
    if isinstance(release, dict):
        release = json.dumps(release)
    if "azurelinux" not in release.lower():
        raise ValueError("This fixture requires a regular Azure Linux COSI")
    if any(
        name in release.lower()
        for name in (
            "azurelinux-container-host",
            "azure container linux",
            '"variantid": "acl"',
        )
    ):
        raise ValueError("ACL is deliberately excluded from this fixture")
    if info.get("version") != "1.2" or not (info.get("disk") or {}).get("size"):
        raise ValueError("The fixture needs a COSI v1.2 image with disk metadata")
    if info.get("osArch") != "x86_64":
        raise ValueError("The fixture requires an x86_64 payload")
    return info


def stage_cosi(path):
    path = path.resolve(strict=True)
    info = metadata(path)
    destination = IMAGE / "cosi/payload.cosi"
    destination.parent.mkdir(parents=True, exist_ok=True)
    if path != destination:
        shutil.copy2(path, destination)
    WORK.mkdir(exist_ok=True)
    (WORK / "payload.json").write_text(
        json.dumps(
            {
                "source": str(path),
                "diskBytes": info["disk"]["size"],
                "recommendedDiskGiB": max(
                    8, (info["disk"]["size"] + GIB - 1) // GIB + 1
                ),
                "osRelease": info["osRelease"],
            },
            indent=2,
        )
        + "\n"
    )
    print(f"Payload: {path} ({info['disk']['size'] // GIB} GiB original disk)")


def build_rpms():
    run(["make", ".cargo/config", "OVERRIDE_RUST_FEED=true"], cwd=REPO)
    version = tomllib.loads((REPO / "crates/trident/Cargo.toml").read_text())[
        "package"
    ]["version"]
    commit = subprocess.check_output(
        ["git", "rev-parse", "--short", "HEAD"], cwd=REPO, text=True
    ).strip()
    release = f"dev.{datetime.date.today():%Y%m%d}99.v{commit}.azl3"
    product_version = f"{version}-{release}"
    run(
        [
            "docker",
            "run",
            "--rm",
            "-e",
            f"TRIDENT_VERSION={product_version}",
            "-v",
            f"{REPO}:/work",
            "-w",
            "/work",
            BUILDER,
            "cargo",
            "build",
            "--locked",
            "--release",
            "--target-dir",
            "target/azl3",
            "--features",
            "trident/dangerous-options,trident/grpc-preview",
            "-p",
            "trident",
            "-p",
            "trident-acl-agent",
            "-p",
            "installer",
        ]
    )
    tag = f"trident-installer-test-rpms:{commit}"
    with tempfile.TemporaryDirectory(prefix="rpm-context-", dir=WORK) as temporary:
        context = Path(temporary)
        shutil.copytree(REPO / "packaging", context / "packaging")
        for name in ["LICENSE", "NOTICE"]:
            shutil.copy2(REPO / name, context / name)
        (context / "bin").mkdir()
        for name in ["trident", "trident-acl-agent", "trident-installer"]:
            shutil.copy2(REPO / "target/azl3/release" / name, context / "bin" / name)
        run(
            [
                "docker",
                "build",
                "-t",
                tag,
                "--build-arg",
                f"TRIDENT_VERSION={product_version}",
                "--build-arg",
                f"RPM_VER={version}",
                "--build-arg",
                f"RPM_REL={release}",
                "-f",
                str(context / "packaging/docker/Dockerfile.azl"),
                str(context),
            ]
        )
        container = subprocess.check_output(
            ["docker", "create", tag], text=True
        ).strip()
        try:
            run(
                [
                    "docker",
                    "cp",
                    f"{container}:/work/trident-rpms.tar.gz",
                    str(context / "rpms.tar.gz"),
                ]
            )
        finally:
            run(["docker", "rm", "-v", container])
        extracted = context / "extracted"
        extracted.mkdir()
        with tarfile.open(context / "rpms.tar.gz") as archive:
            archive.extractall(extracted, filter="data")
        destination = IMAGE / "rpms"
        destination.mkdir(exist_ok=True)
        for old in destination.glob("*.rpm"):
            old.unlink()
        for rpm in extracted.rglob("*.rpm"):
            shutil.copy2(rpm, destination / rpm.name)
        installer = list(destination.glob("trident-installer-*.rpm"))
        if len(installer) != 1:
            raise ValueError("RPM build did not produce exactly one installer package")
        print(f"Installer RPM: {installer[0]}")


def main():
    parser = argparse.ArgumentParser(
        description="Build a Tailor ISO with the current Trident Linux Installer"
    )
    parser.add_argument("--cosi", type=Path, help="Existing non-ACL Azure Linux COSI")
    parser.add_argument(
        "--skip-rpms",
        action="store_true",
        help="Reuse RPMs already staged by this script",
    )
    parser.add_argument(
        "--prepare-only",
        action="store_true",
        help="Stage/validate inputs without building the ISO",
    )
    args = parser.parse_args()
    for tool in ["docker", "tailor", "make"]:
        if not shutil.which(tool):
            parser.error(f"{tool} is required")
    payload = args.cosi or IMAGE / "cosi/payload.cosi"
    if not payload.is_file():
        parser.error("Supply --cosi /path/to/an/existing/non-ACL.cosi")
    stage_cosi(payload)
    if args.prepare_only:
        return
    if not args.skip_rpms:
        build_rpms()
    if not list((IMAGE / "rpms").glob("trident-installer-*.rpm")):
        parser.error("No installer RPM staged; run without --skip-rpms")
    run(["tailor", "--manifest", str(HERE / "tailor.yaml"), "validate"], cwd=HERE)
    run(
        [
            "tailor",
            "--manifest",
            str(HERE / "tailor.yaml"),
            "build",
            "installer",
            "--log-dir",
            str(WORK / "logs"),
        ],
        cwd=HERE,
    )


if __name__ == "__main__":
    main()
