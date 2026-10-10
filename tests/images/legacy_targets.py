#!/usr/bin/env python3
import argparse
import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
WORKSPACE = REPO_ROOT / "tests" / "images" / "tailor.yaml"
LEGACY_MAP = REPO_ROOT / "tests" / "images" / "legacy-map.json"
SELF = Path(__file__).resolve()


def load_map():
    return json.loads(LEGACY_MAP.read_text())


def cargo_cmd() -> str:
    cargo = shutil.which("cargo")
    if cargo:
        return cargo
    fallback = Path.home() / ".cargo" / "bin" / "cargo"
    if fallback.exists():
        return str(fallback)
    alt = Path("/home/bfjelds/.cargo/bin/cargo")
    if alt.exists():
        return str(alt)
    raise SystemExit("cargo not found")


def default_container_full() -> str:
    text = WORKSPACE.read_text()
    needle = """  entries:\n    - name: ic\n      container: """
    idx = text.find(needle)
    if idx == -1:
        raise SystemExit("default toolchain not found in tests/images/tailor.yaml")
    rest = text[idx + len(needle) :].splitlines()
    container = rest[0].strip()
    tag = None
    for line in rest[1:6]:
        s = line.strip()
        if s.startswith("tag:"):
            tag = s.split(":", 1)[1].strip()
            break
    if tag:
        return f"{container}:{tag}"
    return container


def tailor_manifest_for_container(container_ref: str | None) -> Path:
    if not container_ref or container_ref == default_container_full():
        return WORKSPACE
    text = WORKSPACE.read_text()
    name = "ic-override"
    if ":" in container_ref.rsplit("/", 1)[-1]:
        container, tag = container_ref.rsplit(":", 1)
        tag_block = f"      tag: {tag}\n"
    else:
        container = container_ref
        tag_block = ""
    needle = """toolchains:\n  default: ic\n  entries:\n    - name: ic\n      container: mcr.microsoft.com/azurelinux/imagecustomizer\n      tag: latest\n"""
    replacement = (
        "toolchains:\n"
        f"  default: {name}\n"
        "  entries:\n"
        "    - name: ic\n"
        "      container: mcr.microsoft.com/azurelinux/imagecustomizer\n"
        "      tag: latest\n"
        f"    - name: {name}\n"
        f"      container: {container}\n"
        f"{tag_block}"
        "      pull: never\n"
    )
    rendered = text.replace(needle, replacement, 1)
    tmp = REPO_ROOT / "tests" / "images" / ".tailor.make.yaml"
    tmp.write_text(rendered)
    return tmp


def selector_args(entry: dict) -> list[str]:
    args: list[str] = []
    for key, value in entry["selectors"].items():
        args.extend(["-s", f"{key}={value}"])
    return args


def call_tailor(*args: str, capture: bool = False) -> subprocess.CompletedProcess:
    cmd = [
        cargo_cmd(),
        "run",
        "--manifest-path",
        "tools/tailor/Cargo.toml",
        "--quiet",
        "--",
        *args,
    ]
    env = dict(os.environ)
    env["PATH"] = f"{Path.home() / '.cargo' / 'bin'}:{env.get('PATH','')}"
    return subprocess.run(
        cmd, cwd=str(REPO_ROOT), check=True, text=True, capture_output=capture, env=env
    )


def legacy_list(filter_type: str):
    for name, entry in load_map().items():
        if entry["ext"] == filter_type:
            print(name)


def dependencies(name: str):
    mapping = load_map()
    entry = mapping[name]
    image_dir = REPO_ROOT / "tests" / "images" / entry["image"]
    deps: list[Path] = [WORKSPACE, LEGACY_MAP, SELF]
    base_dep_map = {
        "baremetal": REPO_ROOT / "artifacts" / "baremetal.vhdx",
        "core_selinux": REPO_ROOT / "artifacts" / "core_selinux.vhdx",
        "core_arm64": REPO_ROOT / "artifacts" / "core_arm64.vhdx",
        "qemu_guest": REPO_ROOT / "artifacts" / "qemu_guest.vhdx",
    }
    base_dep = base_dep_map.get(entry.get("baseImage"))
    if base_dep is not None:
        deps.append(base_dep)
    for path in sorted(image_dir.rglob("*")):
        if (
            path.is_file()
            and ".rendered" not in path.parts
            and path.name != ".tailor.make.yaml"
        ):
            deps.append(path)
    needs_rpms = name not in {
        "trident-functest",
        "trident-container-installer",
        "trident-container-testimage",
        "trident-container-verity-testimage",
        "trident-container-usrverity-testimage",
        "ubuntu-direct-streaming-testimage-2204-amd64",
        "ubuntu-direct-streaming-testimage-2204-arm64",
        "ubuntu-direct-streaming-testimage-2404-amd64",
        "ubuntu-direct-streaming-testimage-2404-arm64",
        "gb200-direct-streaming-testimage-2404-arm64",
    }
    if needs_rpms:
        rpm_dir = REPO_ROOT / "bin" / "RPMS"
        if rpm_dir.exists():
            deps.append(rpm_dir)
            deps.extend(sorted(rpm_dir.rglob("*.rpm")))
    if entry["image"] == "trident-installer":
        for extra in [
            REPO_ROOT / "bin" / "rcp-agent",
            REPO_ROOT / "tools" / "cmd" / "rcp-agent" / "rcp-agent.service",
        ]:
            if extra.exists():
                deps.append(extra)
    if name == "azl-installer":
        for extra in [
            REPO_ROOT
            / "tests"
            / "images"
            / "azl-installer"
            / "iso"
            / "bin"
            / "liveinstaller",
            REPO_ROOT
            / "tests"
            / "images"
            / "azl-installer"
            / "iso"
            / "images"
            / "trident-testimage.cosi",
        ]:
            if extra.exists():
                deps.append(extra)
    seen = set()
    for dep in deps:
        try:
            s = str(dep.relative_to(REPO_ROOT))
        except ValueError:
            s = str(dep)
        if s not in seen:
            seen.add(s)
            print(s)


def resolve_slug(entry: dict, manifest: Path) -> str:
    proc = call_tailor(
        "--manifest",
        str(manifest),
        "matrix",
        entry["image"],
        "--format",
        "json",
        *selector_args(entry),
        capture=True,
    )
    cells = json.loads(proc.stdout)
    if len(cells) != 1:
        raise SystemExit(
            f"expected exactly one cell for {entry['image']}, got {len(cells)}"
        )
    return cells[0]["slug"]


def build(name: str, output_path: str, container: str | None):
    mapping = load_map()
    entry = mapping[name]
    target = Path(output_path)
    target.parent.mkdir(parents=True, exist_ok=True)
    manifest = tailor_manifest_for_container(container)
    try:
        slug = resolve_slug(entry, manifest)
        call_tailor(
            "--manifest",
            str(manifest),
            "build",
            entry["image"],
            *selector_args(entry),
            "--output-dir",
            str(target.parent),
        )
        source_ext = entry.get("tailorExt", entry["ext"])
        built = target.parent / f"{slug}.{source_ext}"
        if not built.exists():
            raise SystemExit(f"expected built artifact not found: {built}")
        built.replace(target)
    finally:
        if manifest.name == ".tailor.make.yaml":
            manifest.unlink(missing_ok=True)


def oras_download(name: str, out_path: Path):
    ref = f"mcr.microsoft.com/azurelinux/3.0/image/{name}:latest"
    with tempfile.TemporaryDirectory() as td:
        subprocess.run(
            ["oras", "pull", ref, "--output", td, "--platform", "linux/amd64"],
            check=True,
        )
        files = list(Path(td).glob("*.vhdx"))
        if len(files) != 1:
            raise SystemExit(f"expected one .vhdx from {ref}, got {len(files)}")
        out_path.parent.mkdir(parents=True, exist_ok=True)
        shutil.move(str(files[0]), str(out_path))


def download_image(name: str):
    target_map = {
        "baremetal": REPO_ROOT / "artifacts" / "baremetal.vhdx",
        "core_selinux": REPO_ROOT / "artifacts" / "core_selinux.vhdx",
        "minimal": REPO_ROOT / "artifacts" / "minimal.vhdx",
        "minimal_aarch64": REPO_ROOT / "artifacts" / "minimal_aarch64.vhdx",
    }
    if name not in target_map:
        raise SystemExit(f"unsupported base image: {name}")
    oras_download(name, target_map[name])


def main():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)

    p = sub.add_parser("list")
    p.add_argument("--filter-type", required=True)

    p = sub.add_parser("dependencies")
    p.add_argument("name")

    p = sub.add_parser("build")
    p.add_argument("name")
    p.add_argument("--output-path", required=True)
    p.add_argument("--container")

    p = sub.add_parser("show-artifact")
    p.add_argument("item")

    p = sub.add_parser("download-image")
    p.add_argument("name")

    args = ap.parse_args()
    if args.cmd == "list":
        legacy_list(args.filter_type)
    elif args.cmd == "dependencies":
        dependencies(args.name)
    elif args.cmd == "build":
        build(args.name, args.output_path, args.container)
    elif args.cmd == "show-artifact":
        if args.item != "customizer-container-full":
            raise SystemExit(f"unsupported artifact item: {args.item}")
        print(default_container_full())
    elif args.cmd == "download-image":
        download_image(args.name)


if __name__ == "__main__":
    import os

    main()
