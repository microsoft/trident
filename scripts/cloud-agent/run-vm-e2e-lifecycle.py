#!/usr/bin/env python3

import argparse
import importlib.util
import json
import os
import re
import shutil
import signal
import socket
import stat
import subprocess
import sys
import uuid
import xml.etree.ElementTree as ET

from collections import Counter
from contextlib import contextmanager
from pathlib import Path

SPEC = importlib.util.spec_from_file_location(
    "vm_install", Path(__file__).with_name("run-vm-e2e-install.py")
)
shared = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(shared)
require = shared.require
SCRIPT = Path(".pipelines/templates/stages/testing_common/scripts/trigger-rollback.sh")
SOURCE_SCRIPT = Path(__file__).resolve().parents[2] / SCRIPT
IMAGES = {
    "qemu": "trident-vm-grub-verity-testimage",
    "uki": "trident-vm-usr-verity-testimage",
}
CASES = {
    "servicing": (
        "publish-sig-image",
        "deploy-vm",
        "check-deployment",
        "update-loop",
        "rollback",
        "collect-logs",
        "cleanup-vm",
    ),
    "rollback": (
        "prepare-qcow2",
        "deploy-vm",
        "check-deployment",
        "multi-rollback",
        "skip-to-ab-rollback",
        "split-rollback",
        "collect-logs",
        "cleanup-vm",
    ),
}
LANES = tuple(f"{suite}-{flavor}" for suite in CASES for flavor in IMAGES)
STAGES = ("PrepareSSHKeys", "BuildImagesQemu", "BuildImagesUKI")
OUTCOMES = ("passed", "failed", "skipped", "unavailable", "not_run")
TOOL_PATH = "lifecycle-bin/storm-trident"
OWNER_LABEL = "com.microsoft.trident.lifecycle-owner"
OWNER_FORMAT = (
    '{"Id":{{json .Id}},"Name":{{json .Name}},"Owner":'
    '{{json (index .Config.Labels "' + OWNER_LABEL + '")}}}'
)


class StepFailure(ValueError):
    def __init__(self, message, status):
        super().__init__(f"{message}: {status}")
        self.status = status


def checked(status, message):
    if status:
        raise StepFailure(message, status)


@contextmanager
def private_creation():
    previous = os.umask(0o077)
    try:
        yield
    finally:
        os.umask(previous)


def qemu_searchable(path, identity):
    info = path.lstat()
    require(
        stat.S_ISDIR(info.st_mode) and info.st_uid == os.geteuid(),
        "QEMU search access is restricted to newly owned output directories",
    )
    os.chown(path, -1, identity.pw_gid, follow_symlinks=False)
    path.chmod(0o710)


def image_path(flavor, role):
    extension = "qcow2" if role == "base" else "cosi"
    return (
        f"artifacts/test-image/lifecycle/{flavor}/{role}/{IMAGES[flavor]}.{extension}"
    )


def verify_lifecycle(bundle, manifest, args):
    lifecycle = manifest["lifecycle"]
    require(
        set(lifecycle)
        == {
            "schemaVersion",
            "stages",
            "review",
            "imageCustomizerImage",
            "rollbackScriptSha256",
            "toolRevision",
            "toolSha256",
            "toolProducer",
        },
        "Unexpected lifecycle family fields",
    )
    require(lifecycle["schemaVersion"] == 1, "Unsupported lifecycle family schema")
    require(
        manifest["producer"]["definitionId"] == 5073, "Unexpected lifecycle producer"
    )
    require(
        set(lifecycle["stages"]) == set(STAGES)
        and len(set(lifecycle["stages"].values())) == len(STAGES)
        and all(
            re.fullmatch(r"[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}", value)
            for value in lifecycle["stages"].values()
        ),
        "Missing reviewed successful lifecycle producer stage IDs",
    )
    require(
        set(lifecycle["review"]) == {"reviewer", "reference"},
        "Unexpected review fields",
    )
    for field in ("reviewer", "reference"):
        require(
            re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._/@:+-]*", lifecycle["review"][field]),
            "Missing minimal independent lifecycle review identity",
        )
    files = manifest["files"]
    require(
        re.fullmatch(r"[0-9a-f]{40}", args.expected_tool_revision)
        and lifecycle["toolRevision"] == args.expected_tool_revision,
        "Expected lifecycle Storm source revision must be an independently pinned full SHA",
    )
    require(
        re.fullmatch(r"[0-9a-f]{64}", args.tool_sha256)
        and lifecycle["toolSha256"] == args.tool_sha256
        and files[TOOL_PATH]["sha256"] == args.tool_sha256
        and files[TOOL_PATH]["artifact"] == "lifecycle-go-tools"
        and os.access(bundle / TOOL_PATH, os.X_OK),
        "Lifecycle Storm binary pin/artifact mismatch",
    )
    producer = lifecycle["toolProducer"]
    require(
        set(producer)
        == {"organization", "projectId", "definitionId", "buildId", "sourceVersion"}
        and producer["organization"] == "mariner-org"
        and producer["projectId"] == shared.PROJECT_ID
        and producer["sourceVersion"] == args.expected_tool_revision
        and all(
            type(producer[key]) is int and producer[key] > 0
            for key in ("definitionId", "buildId")
        ),
        "Missing immutable separately reviewed lifecycle tool producer",
    )
    for name in ("bin/storm-trident", "bin/netlisten", "ssh/id_rsa.pub"):
        artifact = "ssh-keys" if name.startswith("ssh/") else "go-tools"
        require(files[name]["artifact"] == artifact, f"Artifact mismatch: {name}")
        if name.startswith("bin/"):
            require(os.access(bundle / name, os.X_OK), f"Tool not executable: {name}")
    for flavor in IMAGES:
        for role in ("base", "update-a", "update-b"):
            name = image_path(flavor, role)
            require(
                files[name]["artifact"] == f"image-{flavor}-{role}",
                f"Lifecycle artifact mismatch: {name}",
            )
        require(
            files[image_path(flavor, "update-a")]["sha256"]
            != files[image_path(flavor, "update-b")]["sha256"],
            f"Identical update A/B images: {flavor}",
        )
    require(
        re.fullmatch(
            r"mcr\.microsoft\.com/azurelinux/imagecustomizer@sha256:[0-9a-f]{64}",
            lifecycle["imageCustomizerImage"],
        ),
        "Lifecycle Image Customizer must be an explicit MCR digest, not a tag",
    )
    require(
        shared.digest(SOURCE_SCRIPT) == lifecycle["rollbackScriptSha256"],
        "Servicing rollback support script hash mismatch",
    )
    require(
        {p.name for p in (bundle / "ssh").iterdir()} == {"id_rsa.pub"},
        "Only the public key belongs in bundle/ssh; keep private keys outside",
    )


def check_ports():
    for port in (8000, 8001):
        with socket.socket() as connection:
            connection.bind(("0.0.0.0", port))


def check_host(args):
    require(
        args.exclusive_disposable_host
        and socket.gethostname() == args.expected_hostname,
        "Explicit exclusive disposable host acknowledgement/hostname mismatch",
    )
    require(os.geteuid() == 0, "Run as root, matching the existing QEMU caller")
    shared.check_host()
    for tool in ("virt-install", "ssh", "scp", "ssh-keygen", "lsof", "setpriv"):
        require(shutil.which(tool), f"Missing lifecycle prerequisite: {tool}")
    if args.lane in (None, "rollback-uki"):
        for tool in ("docker", "mksquashfs"):
            require(shutil.which(tool), f"Missing UKI rollback prerequisite: {tool}")
    check_ports()


def run(argv, cwd, log, args, cleanup=False):
    return shared.command(
        argv,
        cwd,
        log,
        args.cleanup_grace_seconds if cleanup else args.timeout_seconds,
        args.cleanup_grace_seconds,
    )


def domain_absent(name, cwd, log, args):
    status = run(
        ["virsh", "-c", "qemu:///system", "domuuid", name], cwd, log, args, cleanup=True
    )
    if status not in (0, 1):
        raise StepFailure("Domain absence query failed", status)
    require(
        status == 1 and "Domain not found" in log.read_text(),
        "Cannot prove suite domain absent (existing domain, libvirt failure or locale)",
    )


def container_ids(name, work, label, args):
    log = work / f"{label}.log"
    checked(
        run(
            [
                "docker",
                "container",
                "ls",
                "--all",
                "--no-trunc",
                "--filter",
                f"name=^/{name}$",
                "--format",
                "{{.ID}}",
            ],
            work,
            log,
            args,
            cleanup=True,
        ),
        "Exact container-name query failed",
    )
    ids = log.read_text().split()
    require(
        len(ids) <= 1 and all(re.fullmatch(r"[0-9a-f]{64}", item) for item in ids),
        "Invalid exact-name Docker identity response",
    )
    return ids


def cleanup_container(args, work, row, completed):
    intent = row["container"]
    # A terminated client does not acknowledge the daemon's accepted create.
    # Snapshots, including repeated empty ones, cannot close that boundary.
    intent["requiresHostDisposal"] = not completed
    intent["outcome"] = "uncertain" if not completed else "pending"
    if not completed:
        intent["uncertainty"] = (
            "Accepted daemon create may register late; outer host disposal required before reuse"
        )
    try:
        ids = container_ids(intent["name"], work, "container-before-cleanup", args)
        if ids:
            container_id = ids[0]
            log = work / "container-identity.log"
            checked(
                run(
                    [
                        "docker",
                        "container",
                        "inspect",
                        container_id,
                        "--format",
                        OWNER_FORMAT,
                    ],
                    work,
                    log,
                    args,
                    cleanup=True,
                ),
                "Container ownership inspection failed",
            )
            identity = json.loads(log.read_text())
            require(
                identity["Id"] == container_id
                and identity["Name"] == "/" + intent["name"]
                and identity["Owner"] == intent["name"],
                "Container ownership mismatch; refusing removal",
            )
            intent["observedId"] = container_id
            checked(
                run(
                    ["docker", "container", "rm", "--force", container_id],
                    work,
                    work / "container-remove.log",
                    args,
                    cleanup=True,
                ),
                "Owned immutable container-ID removal failed",
            )
        require(
            not container_ids(intent["name"], work, "container-after-cleanup", args),
            "Owned container remains after cleanup",
        )
        intent["absenceObserved"] = True
        if completed:
            intent["outcome"] = "clean"
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError):
        intent.update(outcome="uncertain", requiresHostDisposal=True)
        raise
    finally:
        row["requiresHostDisposal"] = intent["requiresHostDisposal"]
        (work / "container-intent.json").write_text(json.dumps(intent, indent=2) + "\n")


def preflight(args, bundle, manifest, output):
    import pwd

    require(
        callable(getattr(shared, "cleanup_signals", None)),
        "Integration prerequisite: approved shared cleanup_signals and default group reaping",
    )
    check_host(args)
    key = args.ssh_private_key
    require(not key.is_symlink(), "SSH private key must not be a symlink")
    key = key.resolve()
    require(
        not key.is_relative_to(bundle) and not key.is_relative_to(output),
        "Keep private key outside bundle and output",
    )
    info = key.stat()
    require(
        stat.S_ISREG(info.st_mode) and info.st_size > 0 and not info.st_mode & 0o077,
        "SSH private key must be a nonempty private regular file (mode 0600 or stricter)",
    )
    public = output / "ssh-public-check.log"
    status = run(["ssh-keygen", "-y", "-P", "", "-f", str(key)], output, public, args)
    checked(status, "SSH key verification failed")
    expected = (bundle / "ssh/id_rsa.pub").read_text().split()
    actual = public.read_text().split()
    require(
        len(expected) >= 2 and actual[:2] == expected[:2],
        "SSH private key does not match cohort public key",
    )
    if args.lane in (None, "rollback-uki"):
        image = manifest["lifecycle"]["imageCustomizerImage"]
        log = output / "image-customizer-check.log"
        status = run(
            ["docker", "image", "inspect", image, "--format", "{{json .RepoDigests}}"],
            output,
            log,
            args,
        )
        checked(status, "Pinned Image Customizer unavailable locally")
        require(
            image in json.loads(log.read_text()), "Image Customizer digest mismatch"
        )
    identity = pwd.getpwnam(args.qemu_user)
    require(
        identity.pw_uid > 0 and identity.pw_gid > 0,
        "QEMU must use an existing nonroot UID and nonroot primary group",
    )
    qemu_searchable(output, identity)
    status = run(
        [
            "setpriv",
            "--reuid",
            str(identity.pw_uid),
            "--regid",
            str(identity.pw_gid),
            "--clear-groups",
            sys.executable,
            "-c",
            "import os,sys; sys.exit(0 if os.access(sys.argv[1], os.X_OK) else 1)",
            str(output),
        ],
        output,
        output / "qemu-access-check.log",
        args,
    )
    checked(
        status,
        "QEMU cannot search output/ancestors; select an already searchable parent",
    )
    if args.lane in (None, "rollback-uki"):
        log = output / "lifecycle-tool-help.log"
        checked(
            run(
                [str(bundle / TOOL_PATH), "run", "rollback", "--", "--help"],
                output,
                log,
                args,
            ),
            "Pinned lifecycle Storm capability check failed",
        )
        require(
            "--image-customizer-container-name" in log.read_text(),
            "Pinned lifecycle Storm lacks the owned-container hook",
        )
    return key, identity


def stage(bundle, work, suite, flavor, identity):
    qemu_searchable(work, identity)
    artifacts = work / "artifacts"
    artifacts.mkdir()
    qemu_searchable(artifacts, identity)
    for role in ("base", "update-a", "update-b"):
        if suite == "rollback" and role == "update-b":
            continue
        source = bundle / image_path(flavor, role)
        target = (
            artifacts if role == "base" or suite == "rollback" else artifacts / role
        )
        target.mkdir(exist_ok=True)
        destination = target / source.name
        shutil.copy2(source, destination)
        destination.chmod(0o600)
    (work / "bin").mkdir()
    for tool in ("storm-trident", "netlisten"):
        source = (
            bundle / TOOL_PATH if tool == "storm-trident" else bundle / "bin" / tool
        )
        shutil.copy2(source, work / "bin" / tool)
    if suite == "servicing":
        (work / SCRIPT).parent.mkdir(parents=True)
        shutil.copy2(SOURCE_SCRIPT, work / SCRIPT)
    return artifacts


def suite_command(work, suite, flavor, name, key, bundle, manifest):
    argv = [
        str(work / "bin/storm-trident"),
        "run",
        suite,
        "-a",
        "-o",
        str(work / "storm"),
        "-j",
        str(work / "storm.junit.xml"),
        "--",
        "--artifacts-dir",
        str(work / "artifacts"),
        "--output-path",
        str(work / "logs"),
        "--platform",
        "qemu",
        "--name",
        name,
        "--serial-log",
        f"/tmp/{name}.log",
        "--ssh-private-key-path",
        str(key),
        "--ssh-public-key-path",
        str(bundle / "ssh/id_rsa.pub"),
        "--force-cleanup",
        "--verbose",
    ]
    if flavor == "qemu":
        argv += ["--secure-boot"]
    if suite == "servicing":
        argv += [
            "--rollback",
            "--retry-count",
            "3",
            "--rollback-retry-count",
            "3",
            "--build-id",
            str(manifest["producer"]["buildId"]),
            "--use-private-ip-address",
        ]
    else:
        argv += ["--uki"]
        if flavor == "qemu":
            argv += [
                "--skip-extension-testing",
                "--skip-runtime-updates",
                "--skip-netplan-runtime-testing",
            ]
        else:
            argv += [
                "--image-customizer-image",
                manifest["lifecycle"]["imageCustomizerImage"],
                "--image-customizer-container-name",
                name + "-mic",
            ]
    return argv


def reports(row, work, suite, flavor, status):
    cases = shared.case_results(work / "storm.junit.xml")
    by_name = {c["name"]: c for c in cases}
    row["cases"] = [
        by_name.get(name, {"name": name, "outcome": "not_run"}) for name in CASES[suite]
    ]
    require(
        Counter(c["name"] for c in cases) == Counter(CASES[suite]),
        "Storm report differs from the selected suite case inventory",
    )
    skips = {"publish-sig-image"} if suite == "servicing" else set()
    if suite == "rollback" and flavor == "qemu":
        skips.add("skip-to-ab-rollback")
    if status == 0:
        require(
            all(
                c["outcome"] == ("skipped" if c["name"] in skips else "passed")
                for c in row["cases"]
            ),
            "Storm success did not execute all enabled suite cases",
        )


def execute_lane(args, bundle, manifest, output, key, identity, row):
    suite, flavor = row["lane"].split("-")
    work = output / row["lane"]
    work.mkdir()
    stage(bundle, work, suite, flavor, identity)
    name = "trident-lifecycle-" + uuid.uuid4().hex
    row["domain"] = name
    domain_absent(name, work, work / "domain-before.log", args)
    require(not Path(f"/tmp/{name}.log").exists(), "Suite serial path already exists")
    check_ports()
    if suite == "rollback" and flavor == "uki":
        row["container"] = {
            "name": name + "-mic",
            "ownerLabel": OWNER_LABEL + "=" + name + "-mic",
            "createMayHaveBeenAccepted": False,
            "outcome": "not_run",
            "requiresHostDisposal": False,
        }
        intent_path = work / "container-intent.json"
        intent_path.write_text(json.dumps(row["container"], indent=2) + "\n")
        require(
            not container_ids(
                row["container"]["name"], work, "container-preflight", args
            ),
            "Container name already exists; refusing execution or deletion",
        )
        status = run(
            [
                str(work / "bin/storm-trident"),
                "script",
                "build-extension-images",
                "--build-sysexts",
                "--num-clones",
                "3",
            ],
            work / "artifacts",
            work / "extensions.log",
            args,
        )
        row["extensionExitCode"] = status
        if status:
            row["outcome"] = "unavailable"
            return status
        row["extensions"] = {}
        for index in range(1, 4):
            path = work / "artifacts" / f"test-sysext-{index}.raw"
            require(
                path.is_file() and path.stat().st_size > 0, "Missing generated sysext"
            )
            row["extensions"][path.name] = shared.digest(path)
    argv = suite_command(work, suite, flavor, name, key, bundle, manifest)
    row.update(command=argv, executed=True, outcome="failed")
    status = 0
    try:
        if "container" in row:
            row["container"]["createMayHaveBeenAccepted"] = True
            row["container"]["outcome"] = "pending"
            intent_path.write_text(json.dumps(row["container"], indent=2) + "\n")
        status = run(argv, work, work / "console.log", args)
        row["suiteExitCode"] = status
        try:
            reports(row, work, suite, flavor, status)
        except (OSError, ValueError, KeyError, ET.ParseError) as error:
            row["reportError"] = str(error)
            status = status or 1
    except KeyboardInterrupt:
        status = status or 130
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        row["error"] = str(error)
        status = status or 1
    finally:
        # Rollback's CleanupVM does not remove a QEMU domain. Reuse servicing's
        # named cleanup case for both suites, without touching other domains.
        with shared.cleanup_signals():
            if "container" in row:
                try:
                    cleanup_container(args, work, row, completed=status == 0)
                except (
                    OSError,
                    ValueError,
                    KeyError,
                    TypeError,
                    subprocess.SubprocessError,
                ) as error:
                    row["containerCleanupError"] = str(error)
                    status = status or getattr(error, "status", 1)
            try:
                cleanup = [
                    str(work / "bin/storm-trident"),
                    "run",
                    "servicing",
                    "-a",
                    "-o",
                    str(work / "cleanup-storm"),
                    "-j",
                    str(work / "cleanup.junit.xml"),
                    "--",
                    "--platform",
                    "qemu",
                    "--name",
                    name,
                    "--output-path",
                    str(work / "cleanup-logs"),
                    "--test-case-to-run",
                    "cleanup-vm",
                ]
                row["cleanupExitCode"] = run(
                    cleanup, work, work / "cleanup.log", args, cleanup=True
                )
                checked(row["cleanupExitCode"], "Suite-owned cleanup failed")
                domain_absent(name, work, work / "domain-after.log", args)
                serial = Path(f"/tmp/{name}.log")
                if serial.is_file() and not serial.is_symlink():
                    shutil.copy2(serial, work / "serial.log")
            except (OSError, ValueError, subprocess.SubprocessError) as error:
                row["cleanupError"] = str(error)
                status = status or getattr(error, "status", 1)
    row["exitCode"] = status
    if status == 0:
        row["outcome"] = "passed"
    return status


def execute(args):
    with private_creation():
        return execute_private(args)


def execute_private(args):
    bundle, output = args.bundle.resolve(), args.output.resolve()
    require(
        not output.is_relative_to(bundle) and not bundle.is_relative_to(output),
        "Output and bundle must not overlap",
    )
    output.mkdir(exist_ok=False, mode=0o700)
    require(
        "system.posix_acl_default" not in os.listxattr(output),
        "Output inherited a default ACL; select a parent without default ACL inheritance",
    )
    result = {
        "suite": "qemu-servicing-rollback",
        "candidateRevision": args.expected_revision,
        "manifestSha256": args.manifest_sha256,
        "toolRevision": args.expected_tool_revision,
        "toolSha256": args.tool_sha256,
        "outcome": "unavailable",
        "lanes": [
            {
                "lane": lane,
                "selected": args.lane in (None, lane),
                "executed": False,
                "outcome": "not_run",
                "cases": [
                    {"name": name, "outcome": "not_run"}
                    for name in CASES[lane.split("-")[0]]
                ],
            }
            for lane in LANES
        ]
        + [
            {
                "lane": "rollback-qemu-grub",
                "selected": False,
                "executed": False,
                "outcome": "not_run",
                "cases": [],
                "reason": "Excluded: actual e2e-template.yml sets includeQemuGrub=false",
            }
        ],
    }
    status = 0
    try:
        require(args.lane is None or args.lane in LANES, "Unsupported lifecycle lane")
        manifest = shared.verify_bundle(
            bundle,
            args.manifest.resolve(),
            args.manifest_sha256,
            args.expected_revision,
        )
        verify_lifecycle(bundle, manifest, args)
        result["provenance"] = manifest
        with shared.host_lock():
            key, identity = preflight(args, bundle, manifest, output)
            result["qemuIdentity"] = {
                "user": identity.pw_name,
                "uid": identity.pw_uid,
                "gid": identity.pw_gid,
            }
            for row in result["lanes"]:
                if not row["selected"]:
                    continue
                try:
                    status = execute_lane(
                        args, bundle, manifest, output, key, identity, row
                    )
                except KeyboardInterrupt:
                    status = 130
                    row.update(outcome="unavailable", error="Interrupted before suite")
                except (
                    OSError,
                    ValueError,
                    KeyError,
                    TypeError,
                    subprocess.SubprocessError,
                ) as error:
                    status = getattr(error, "status", 1)
                    row.update(outcome="unavailable", error=str(error))
                row["exitCode"] = status
                if status:
                    break
    except KeyboardInterrupt:
        status = status or 130
        result["error"] = "Interrupted"
    except (
        OSError,
        ValueError,
        KeyError,
        TypeError,
        subprocess.SubprocessError,
    ) as error:
        result["error"] = str(error)
        status = getattr(error, "status", status or 1)
    finally:
        for row in result["lanes"]:
            if row["selected"] and not row["executed"] and "error" in result:
                row["outcome"] = "unavailable"
        result["exitCode"] = status
        result["requiresHostDisposal"] = any(
            row.get("requiresHostDisposal", False) for row in result["lanes"]
        )
        result["outcome"] = (
            "passed"
            if not status
            else (
                "failed"
                if any(r["executed"] for r in result["lanes"])
                else "unavailable"
            )
        )
        counts = Counter(row["outcome"] for row in result["lanes"])
        result["counts"] = {outcome: counts[outcome] for outcome in OUTCOMES}
        result["selected"] = sum(row["selected"] for row in result["lanes"])
        result["executed"] = sum(row["executed"] for row in result["lanes"])
        counts = Counter(c["outcome"] for row in result["lanes"] for c in row["cases"])
        result["caseCounts"] = {outcome: counts[outcome] for outcome in OUTCOMES}
        (output / "results.json").write_text(json.dumps(result, indent=2) + "\n")
    return status


def main():
    parser = argparse.ArgumentParser(
        description="Existing QEMU servicing/rollback suites"
    )
    for name in ("bundle", "manifest", "output", "ssh-private-key"):
        parser.add_argument("--" + name, type=Path, required=True)
    for name in (
        "manifest-sha256",
        "expected-revision",
        "expected-hostname",
        "expected-tool-revision",
        "tool-sha256",
    ):
        parser.add_argument("--" + name, required=True)
    parser.add_argument(
        "--exclusive-disposable-host", action="store_true", required=True
    )
    parser.add_argument("--lane", choices=LANES)
    parser.add_argument(
        "--qemu-user",
        default="libvirt-qemu",
        help="Existing configured nonroot libvirt QEMU account (no account changes)",
    )
    for name in ("timeout-seconds", "cleanup-grace-seconds"):
        parser.add_argument("--" + name, type=shared.positive_seconds, required=True)
    args = parser.parse_args()
    signal.signal(signal.SIGTERM, shared.interrupt)
    try:
        return execute(args)
    except (OSError, ValueError) as error:
        print(error, file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
