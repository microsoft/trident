#!/usr/bin/env python3

import argparse
import hashlib
import json
import math
import os
import platform
import re
import shutil
import signal
import stat
import subprocess
import sys
import xml.etree.ElementTree as ET

from contextlib import contextmanager
from pathlib import Path, PurePosixPath

PROJECT_ID = "2311650c-e79e-4301-b4d2-96543fdd84ff"
HOST_LOCK = Path("/var/lib/trident-cloud-agent/functional.lock")
MATRIX_PREFIX = "##vso[task.setvariable variable=TEST_MATRIX_E2E_VM_HOST;isOutput=true]"


def require(condition, message):
    if not condition:
        raise ValueError(message)


def digest(path):
    result = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            result.update(chunk)
    return result.hexdigest()


def bundle_path(bundle, name):
    path = PurePosixPath(name)
    require(
        name and "\\" not in name and not path.is_absolute() and ".." not in path.parts,
        f"Unsafe bundle path: {name}",
    )
    target = bundle.joinpath(*path.parts)
    require(target.resolve().is_relative_to(bundle), f"Path escapes bundle: {name}")
    return target


def verify_bundle(bundle, manifest_path, manifest_hash, revision):
    require(
        re.fullmatch(r"[0-9a-f]{40}", revision), "Expected revision must be full SHA"
    )
    require(
        re.fullmatch(r"[0-9a-f]{64}", manifest_hash),
        "Expected manifest hash must be SHA256",
    )
    require(digest(manifest_path) == manifest_hash, "Provenance manifest hash mismatch")
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    require(manifest["schemaVersion"] == 1, "Unsupported provenance schema")
    producer = manifest["producer"]
    require(
        producer["organization"] == "mariner-org"
        and producer["projectId"] == PROJECT_ID,
        "Unexpected producer organization/project",
    )
    for field in ("definitionId", "buildId"):
        require(
            type(producer[field]) is int and producer[field] > 0,
            f"Producer {field} must be an immutable positive ID",
        )
    require(
        producer["sourceVersion"] == revision
        and manifest["candidateRevision"] == revision,
        "Artifacts must contain Trident built from the expected candidate revision",
    )
    files = manifest["files"]
    require(isinstance(files, dict) and files, "Missing artifact inventory")
    actual = set()
    for path in bundle.rglob("*"):
        require(not path.is_symlink(), f"Symlinks not allowed in bundle: {path}")
        if path.is_file() and path.resolve() != manifest_path.resolve():
            actual.add(path.relative_to(bundle).as_posix())
    require(actual == set(files), "Bundle inventory differs from provenance manifest")
    for name, metadata in files.items():
        path = bundle_path(bundle, name)
        require(path.is_file(), f"Missing artifact file: {name}")
        require(
            isinstance(metadata["artifact"], str) and metadata["artifact"].strip(),
            f"Missing pipeline artifact name: {name}",
        )
        require(
            type(metadata["size"]) is int
            and path.stat().st_size == metadata["size"]
            and metadata["size"] > 0,
            f"Artifact size mismatch: {name}",
        )
        require(digest(path) == metadata["sha256"], f"Artifact hash mismatch: {name}")
    require("bin/storm-trident" in files, "Missing candidate go-tools/storm-trident")
    require(
        files["bin/storm-trident"]["artifact"] == "go-tools",
        "storm-trident must be from go-tools",
    )
    require(
        any(
            name.startswith("artifacts/test-image/") and name.endswith(".cosi")
            for name in files
        ),
        "Missing prepared COSI test images",
    )
    certificate = manifest.get("signingCertificate")
    require(not certificate or certificate in files, "Unverified signing certificate")
    return manifest


def parse_matrix(text):
    require(
        "Unknown test ring" not in text,
        "Unsupported stage type, refusing ring fallback",
    )
    lines = [
        line[len(MATRIX_PREFIX) :]
        for line in text.splitlines()
        if line.startswith(MATRIX_PREFIX)
    ]
    require(len(lines) == 1, "Expected exactly one VM/HOST matrix")
    matrix = json.loads(lines[0])
    require(isinstance(matrix, dict) and matrix, "Empty VM/HOST matrix is not a pass")
    for name, entry in matrix.items():
        require(
            re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_-]*", name)
            and entry["SCENARIO"] == name
            and entry["HARDWARE"] == "vm"
            and entry["RUNTIME"] == "host"
            and entry["tridentDeploymentEnv"] == "virtualMachine"
            and entry["tridentRuntimeEnv"] == "host",
            f"Unsupported matrix entry: {name}",
        )
        require(entry["TEST_RING"] not in ("", "none"), "Disabled test ring")
    return matrix


@contextmanager
def cleanup_signals():
    handlers = {
        sig: signal.signal(sig, signal.SIG_IGN)
        for sig in (signal.SIGINT, signal.SIGTERM)
    }
    try:
        yield
    finally:
        for sig, handler in handlers.items():
            signal.signal(sig, handler)


def reap(process, grace, group=False):
    with cleanup_signals():
        if group:
            signal_group(process, signal.SIGTERM)
        else:
            process.terminate()
        timed_out = False
        try:
            process.wait(timeout=grace)
        except subprocess.TimeoutExpired:
            timed_out = True
        finally:
            # The leader may exit before a descendant, including netlisten.
            # This also applies to Storm's default leader-first cancellation.
            signal_group(process, signal.SIGKILL)
            if timed_out:
                process.wait()


def signal_group(process, sig):
    try:
        os.killpg(process.pid, sig)
    except ProcessLookupError:
        return  # The entire group has already exited.


def command(argv, cwd, log, timeout, grace, group_cleanup=False):
    with log.open("wb") as stream:
        process = subprocess.Popen(
            argv,
            cwd=cwd,
            stdout=stream,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        try:
            status = process.wait(timeout=timeout)
            signal_group(process, signal.SIGKILL)
            return status
        except (subprocess.TimeoutExpired, KeyboardInterrupt) as error:
            # Storm handles SIGTERM by cancelling the suite and running Cleanup.
            # Repeated cancellation must not release the host lock before reaping.
            reap(process, grace, group=group_cleanup)
            return 130 if isinstance(error, KeyboardInterrupt) else 124


@contextmanager
def host_lock():
    import fcntl

    fd = os.open(HOST_LOCK, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    try:
        info = os.fstat(fd)
        require(
            stat.S_ISREG(info.st_mode) and info.st_nlink == 1,
            "Unsafe host execution lock",
        )
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        require(
            os.stat(HOST_LOCK, follow_symlinks=False).st_ino == info.st_ino,
            "Host lock changed during acquisition",
        )
        yield
    finally:
        os.close(fd)


def check_host():
    require(
        platform.system() == "Linux" and platform.machine() == "x86_64",
        "Requires disposable amd64 Ubuntu 22.04 host",
    )
    release = Path("/etc/os-release").read_text()
    require(
        "ID=ubuntu\n" in release and 'VERSION_ID="22.04"' in release,
        "Requires disposable Ubuntu 22.04 host",
    )
    require(os.access("/dev/kvm", os.R_OK | os.W_OK), "KVM is missing or inaccessible")
    for tool in ("sudo", "virsh", "qemu-system-x86_64"):
        require(shutil.which(tool), f"Missing prerequisite tool: {tool}")
    subprocess.run(
        ["virsh", "-c", "qemu:///system", "list", "--all"],
        check=True,
        timeout=30,
        capture_output=True,
    )


def case_results(path):
    cases = []
    for case in ET.parse(path).getroot().iter("testcase"):
        skipped = case.find("skipped")
        if case.find("failure") is not None or case.find("error") is not None:
            outcome = "failed"
        elif skipped is not None:
            outcome = (
                "not_run" if skipped.get("type") in ("NotRun", "Pending") else "skipped"
            )
        else:
            require(case.get("status") == "PASS", "Unknown Storm JUnit case status")
            outcome = "passed"
        cases.append({"name": case.attrib["name"], "outcome": outcome})
    require(cases, "Storm produced no test cases")
    return cases


def execute(args):
    bundle = args.bundle.resolve()
    output = args.output.resolve()
    require(
        not output.is_relative_to(bundle) and not bundle.is_relative_to(output),
        "Output and bundle must not overlap",
    )
    output.mkdir(parents=True, exist_ok=False)
    result = {
        "suite": "storm-vm-host-install-configuration",
        "candidateRevision": args.expected_revision,
        "manifestSha256": args.manifest_sha256,
        "stageType": args.stage_type,
        "outcome": "failed",
        "scenarios": [],
    }
    status = 1
    try:
        manifest = verify_bundle(
            bundle,
            args.manifest.resolve(),
            args.manifest_sha256,
            args.expected_revision,
        )
        result["provenance"] = manifest
        (output / "provenance.json").write_text(json.dumps(manifest, indent=2) + "\n")
        storm = bundle / "bin/storm-trident"
        require(os.access(storm, os.X_OK), "Candidate storm-trident is not executable")
        with host_lock():
            check_host()
            status = command(
                [str(storm), "script", "e2e-matrix", args.stage_type],
                output,
                output / "matrix.log",
                args.timeout_seconds,
                args.cleanup_grace_seconds,
            )
            require(status == 0, f"Storm matrix command failed: {status}")
            matrix = parse_matrix((output / "matrix.log").read_text())
            (output / "matrix.json").write_text(json.dumps(matrix, indent=2) + "\n")
            require(
                not args.scenario or args.scenario in matrix,
                "Requested scenario is not in the generated VM/HOST matrix",
            )
            for name, entry in sorted(matrix.items()):
                result["scenarios"].append(
                    {
                        "scenario": name,
                        "variant": entry,
                        "outcome": "not_run",
                        "selected": not args.scenario or name == args.scenario,
                        "cases": [],
                    }
                )
            # Resolve every selected input before starting any VM.
            for row in result["scenarios"]:
                if not row["selected"]:
                    continue
                entry = row["variant"]
                require(
                    entry["tridentConfigurationName"] != "extensions",
                    "Registry-backed extensions need verified OCI preparation; unsupported here",
                )
                iso = f"artifacts/iso/{entry['installerISOName']}.iso"
                require(iso in manifest["files"], f"Missing candidate installer: {iso}")
                require(
                    manifest["files"][iso]["artifact"] == entry["installerISOName"],
                    f"Installer pipeline artifact mismatch: {iso}",
                )
                bundle_path(bundle, iso)
            for row in result["scenarios"]:
                if not row["selected"]:
                    continue
                scenario_output = output / row["scenario"]
                scenario_output.mkdir()
                images = scenario_output / "test-image"
                shutil.copytree(
                    bundle / "artifacts/test-image", images, copy_function=os.link
                )
                junit = scenario_output / "storm-e2e.junit.xml"
                argv = [
                    str(storm),
                    "run",
                    row["scenario"],
                    "-o",
                    str(scenario_output),
                    "-j",
                    str(junit),
                    "--",
                    "--pipeline-run",
                    "--iso",
                    str(
                        bundle
                        / f"artifacts/iso/{row['variant']['installerISOName']}.iso"
                    ),
                    "--test-image-dir",
                    str(images),
                    "--test-ring",
                    row["variant"]["TEST_RING"],
                    "--logstream-file",
                    str(scenario_output / "logstream-full.log"),
                    "--tracestream-file",
                    str(scenario_output / "tracestream.jsonl"),
                ]
                certificate = manifest.get("signingCertificate")
                if certificate:
                    argv += ["--signing-cert", str(bundle_path(bundle, certificate))]
                row["command"] = argv
                row["outcome"] = "failed"
                status = command(
                    argv,
                    scenario_output,
                    scenario_output / "console.log",
                    args.timeout_seconds,
                    args.cleanup_grace_seconds,
                )
                row["exitCode"] = status
                try:
                    row["cases"] = case_results(junit)
                    if status == 0:
                        require(
                            any(c["outcome"] == "passed" for c in row["cases"])
                            and all(
                                c["outcome"] in ("passed", "skipped")
                                for c in row["cases"]
                            ),
                            "Storm returned success with failed or not-run cases",
                        )
                except (OSError, ValueError, ET.ParseError) as error:
                    row["reportError"] = str(error)
                    status = status or 1
                if status:
                    break
                row["outcome"] = "passed"
            if status == 0:
                result["outcome"] = "passed"
    except (
        OSError,
        ValueError,
        KeyError,
        TypeError,
        subprocess.SubprocessError,
    ) as error:
        result["error"] = str(error)
        status = status or 1
    except KeyboardInterrupt:
        result["error"] = "Interrupted"
        status = 130
    finally:
        result["exitCode"] = status
        (output / "results.json").write_text(json.dumps(result, indent=2) + "\n")
    return status


def positive_seconds(value):
    number = float(value)
    if not math.isfinite(number) or number <= 0:
        raise argparse.ArgumentTypeError("Timeout must be finite and positive")
    return number


def main():
    parser = argparse.ArgumentParser(
        description="Explicit Storm VM/HOST install/configuration"
    )
    parser.add_argument("--bundle", type=Path, required=True)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--manifest-sha256", required=True)
    parser.add_argument("--expected-revision", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--stage-type", required=True)
    parser.add_argument("--scenario", help="Run one member of the generated matrix")
    parser.add_argument(
        "--timeout-seconds",
        type=positive_seconds,
        required=True,
        help="Bound for matrix discovery and each scenario",
    )
    parser.add_argument("--cleanup-grace-seconds", type=positive_seconds, required=True)
    args = parser.parse_args()
    signal.signal(signal.SIGTERM, interrupt)
    try:
        return execute(args)
    except (OSError, ValueError) as error:
        print(error, file=sys.stderr)
        return 1


def interrupt(signum, frame):
    raise KeyboardInterrupt


if __name__ == "__main__":
    sys.exit(main())
