#!/usr/bin/env python3

"""Temporary, packet-gated execution of one existing plain VM/HOST scenario."""

import argparse
import hashlib
import json
import os
import re
import shutil
import signal
import stat
import sys
import tempfile
import xml.etree.ElementTree as ET

from http import HTTPStatus
from pathlib import Path
from types import SimpleNamespace

import inspect_vm_e2e_images as inspection
import prepare_vm_e2e_artifacts as prepare

common = inspection.VM_RUNNER
SOURCE = inspection.REVISION
SCENARIO = "simple_vm-host"
STAGE = "pr-e2e"
REVIEWED_PACKET_SHA256 = (
    "2541fa0bfa52c9b35eae34c22b0e9fa31da57566de4e0b650b44d5991ce3190c"
)
PACKET_LIMIT = 64 * 1024
REPORT_LIMIT = 8192
PREPARATION_EXCEPTION_CLASSES = {
    "ArtifactError",
    "OSError",
    "FileNotFoundError",
    "PermissionError",
    "FileExistsError",
    "IsADirectoryError",
    "NotADirectoryError",
    "TimeoutError",
    "ValueError",
    "JSONDecodeError",
    "UnicodeDecodeError",
    "UnicodeEncodeError",
    "KeyError",
    "TypeError",
    "SubprocessError",
    "CalledProcessError",
    "TimeoutExpired",
    "BadZipFile",
}
PREPARATION_HTTP_STATUSES = frozenset(status.value for status in HTTPStatus)
REQUIRED_CASES = {
    "setup-test-host",
    "install-os",
    "check-trident-ssh",
    "validate-install",
}
DIAGNOSTIC_CASES = REQUIRED_CASES | {
    "install-vm-deps",
    "prepare-hc",
    "prepare-test-images",
    "validate-host-diagnostics",
    "collect-container-logs",
    "capture-install-screenshot",
    "collect-install-boot-metrics",
}
LOG_MARKERS = {
    "permission-denied": (b"permission denied",),
    "missing-command-or-file": (b"command not found", b"no such file or directory"),
    "python-import": (b"modulenotfounderror:", b"importerror:"),
    "apt-package-unavailable": (
        b"unable to locate package",
        b"has no installation candidate",
    ),
    "pip-no-distribution": (b"no matching distribution found",),
    "pip-externally-managed": (b"externally-managed-environment",),
    "authentication-message": (b"authentication failed", b"unauthorized"),
    "access-message": (b"forbidden", b"access denied"),
    "libvirt-connect": (
        b"failed to connect to the hypervisor",
        b"failed to connect socket",
        b"failed to connect to libvirt",
    ),
    "kvm-message": (b"could not access kvm", b"failed to initialize kvm"),
    "timeout-message": (b"timed out", b"timeout expired"),
    "address-in-use": (b"address already in use",),
}
RUNNER_FAILURES = {
    "matrix-command-failed": "Storm matrix command failed:",
    "scenario-not-in-matrix": "Requested scenario is not in the generated VM/HOST matrix",
    "matrix-missing": "Expected exactly one VM/HOST matrix",
    "matrix-empty": "Empty VM/HOST matrix is not a pass",
    "matrix-ring-unsupported": "Unsupported stage type, refusing ring fallback",
    "registry-unsupported": "Registry-backed extensions need verified OCI preparation;",
}
# Exact error prefixes in the pinned product's install.go, netlaunch.go and libvirt.go.
INSTALL_OPERATIONS = {
    "host-config-create": "failed to create temporary host config file: ",
    "host-config-write": "failed to write to temporary host config file: ",
    "host-config-sync": "failed to sync temporary host config file: ",
    "host-config-read": "failed to read prepared host config file: ",
    "serial-monitor-start": "failed to start VM serial monitor: ",
    "installer-iso-read": "failed to read ISO file '",
    "netlaunch-listen": "failed to open port listening on '",
    "outbound-ip-discovery": "failed to find default outbound IP: ",
    "netlaunch-config-read": "failed to read Trident config: ",
    "netlaunch-config-unmarshal": "failed to unmarshal Trident config: ",
    "netlaunch-config-marshal": "failed to marshal Trident config: ",
    "installer-config-patch": "failed to patch Trident config into ISO: ",
    "logstream-setup": "failed to setup logstream: ",
    "tracestream-setup": "failed to setup tracestream: ",
    "signing-certificate-open": "failed to open signing certificate '",
    "vm-initialize": "failed to initialize VM: ",
    "vm-start": "failed to start VM: ",
}
UEFI_OPERATIONS = {
    "uefi-domain-xml-read": "failed to get XML description of domain '",
    "uefi-domain-xml-parse": "failed to parse domain XML: ",
    "uefi-nvram-node-missing": "no <nvram> node found in domain XML",
    "uefi-nvram-remove": "failed to remove existing NVRAM file: ",
    "uefi-domain-redefine": "failed to define domain with XML '",
    "uefi-domain-start-paused": "failed to create domain '",
    "uefi-domain-destroy-paused": "failed to destroy domain '",
    "uefi-virt-fw-vars": "failed to set boot URI: ",
}
INSTALL_MISSING_PATHS = (
    "/usr/bin/qemu-system-x86_64",
    "/usr/share/OVMF/OVMF_CODE_4M.ms.fd",
    "/usr/share/OVMF/OVMF_VARS_4M.ms.fd",
    "/dev/pts/2",
    "/dev/kvm",
    f"/tmp/trident-e2e-{SCENARIO}-vm-0-serial0.log",
    f"/var/lib/trident-e2e-{SCENARIO}-nvram/trident-e2e-{SCENARIO}-vm-0_VARS.fd",
    *(
        f"/var/lib/libvirt/trident-e2e-{SCENARIO}/"
        f"trident-e2e-{SCENARIO}-vm-0-volume-{index}.qcow2"
        for index in (0, 1)
    ),
)
UKI = {
    "combined",
    "memory-constraint-combined",
    "rerun",
    "usr-verity",
    "usr-verity-raid",
}
PLAIN = {
    "base",
    "encrypted-partition",
    "encrypted-raid",
    "encrypted-swap",
    "health-checks-install",
    "misc",
    "raid-mirrored",
    "raid-resync-small",
    "raid-small",
    "root-verity",
    "simple",
    "split",
}
COMMANDS = ("az", "git", "sudo", "bash", "mountpoint", "apt-get", "pip3", "runuser")
ALTERNATE_CREDENTIALS = (
    "AZURE_DEVOPS_EXT_PAT",
    "SYSTEM_ACCESSTOKEN",
    "AZURE_ACCESS_TOKEN",
    "AZURE_CLIENT_SECRET",
    "AZURE_FEDERATED_TOKEN_FILE",
)


def report():
    return {
        "schemaVersion": 1,
        "scope": "initial-plain-install-only-not-full-vm-e2e",
        "outcome": "failed",
        "phase": "publication",
        "sourceRevision": SOURCE,
        "harnessRevision": None,
        "producerBuildId": inspection.BUILD,
        "reviewedDefinitionRevision": 7,
        "packetSha256": None,
        "manifestSha256": None,
        "scenario": SCENARIO,
        "stageType": STAGE,
        "selectedScenarioCount": 1,
        "unselectedScenarioCount": None,
        "unselectedConfigurationCounts": None,
        "caseCounts": None,
        "preparationDiagnostic": None,
        "executionDiagnostic": None,
        "resourceInventoryRestored": False,
        "cleanupBasis": "libvirt-uuid-inventory-only-not-proof-of-host-cleanliness",
        "hostDisposition": "dispose-required-no-further-lanes",
        "notRun": [
            "other-install-configurations",
            "registry-extensions",
            "uki",
            "legacy",
            "lifecycle",
            "acl-agent",
            "container",
            "arm64",
            "azure-platform",
            "bare-metal",
        ],
    }


def publish(output, result):
    text = json.dumps(result, sort_keys=True) + "\n"
    common.require(len(text.encode()) <= REPORT_LIMIT, "report limit")
    inspection.write_report(output, result)


def failed_publication(output, _fact):
    publish(output, report())


def read_bounded(path, limit):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, "rb") as stream:
        info = os.fstat(stream.fileno())
        common.require(
            stat.S_ISREG(info.st_mode) and info.st_size <= limit, "bounded regular file"
        )
        value = stream.read(limit + 1)
    common.require(len(value) <= limit, "file limit")
    return value


def read_json(path, limit):
    value = read_bounded(path, limit)
    return value, json.loads(value)


def preparation_failure(path):
    fact = {"state": "unavailable", "exceptionClass": None, "httpStatus": None}
    try:
        raw = read_bounded(path, 64 * 1024)
        footer = raw.removesuffix(b"\n").rsplit(b"\n", 1)[-1]
        match = re.fullmatch(
            rb"Artifact preparation failed \(([A-Za-z]+)(?:, HTTP ([0-9]{3}))?\)",
            footer,
        )
        if match:
            name = match[1].decode("ascii")
            status = int(match[2]) if match[2] else None
            if name in PREPARATION_EXCEPTION_CLASSES and (
                status is None or status in PREPARATION_HTTP_STATUSES
            ):
                fact.update(state="captured", exceptionClass=name, httpStatus=status)
    except (OSError, ValueError):
        return fact
    return fact


def packet(args, target):
    raw, value = read_json(args.evidence, PACKET_LIMIT)
    common.require(
        args.evidence_sha256 == REVIEWED_PACKET_SHA256
        and hashlib.sha256(raw).hexdigest() == args.evidence_sha256,
        "exact independent packet pin required",
    )
    prepare.keys(
        value, ("schemaVersion", "producer", "review", "logs", "rpms", "artifacts")
    )
    common.require(
        value["schemaVersion"] == 1
        and value["producer"] == prepare.producer(producer_args(target)),
        "packet source/producer mismatch",
    )
    prepare.keys(value["review"], ("reviewer", "reference"))
    for field in ("reviewer", "reference"):
        prepare.identity(value["review"][field])
    # This copy, not the caller's mutable pathname, is given to the existing resolver.
    target.write_bytes(raw)


def producer_args(evidence):
    return SimpleNamespace(
        build_id=inspection.BUILD,
        expected_definition_id=5073,
        expected_revision=SOURCE,
        expected_ref=inspection.REF,
        evidence=evidence,
        include_uki_certificate=False,
    )


def inventory():
    result = {}
    for resource, command in (
        ("domains", "list"),
        ("networks", "net-list"),
        ("pools", "pool-list"),
    ):
        text = inspection.command(
            ["virsh", "-c", "qemu:///system", command, "--all", "--uuid"],
            timeout=30,
        )
        rows = text.split()
        common.require(
            len(rows) <= 64
            and all(re.fullmatch(r"[0-9a-f-]{36}", row) for row in rows),
            "bounded resource inventory required",
        )
        result[resource] = sorted(rows)
    return result


def summarize(path, result):
    _, data = read_json(path, 1024 * 1024)
    common.require(
        data["suite"] == "storm-vm-host-install-configuration"
        and data["candidateRevision"] == SOURCE
        and data["manifestSha256"] == result["manifestSha256"]
        and data["stageType"] == STAGE
        and data["exitCode"] == 0
        and data["outcome"] == "passed",
        "selected execution did not pass",
    )
    rows = data["scenarios"]
    common.require(isinstance(rows, list) and 1 <= len(rows) <= 64, "matrix limit")
    selected = [row for row in rows if row["selected"]]
    common.require(
        len(selected) == 1
        and selected[0]["scenario"] == SCENARIO
        and selected[0]["outcome"] == "passed"
        and all(row["outcome"] == "not_run" for row in rows if not row["selected"]),
        "selection or not-run mismatch",
    )
    row = selected[0]
    variant = row["variant"]
    common.require(
        variant["tridentConfigurationName"] == "simple"
        and variant["installerISOName"] == "trident-installer"
        and variant["HARDWARE"] == "vm"
        and variant["RUNTIME"] == "host"
        and variant["TEST_RING"] == STAGE,
        "unexpected selected variant",
    )
    cases = row["cases"]
    common.require(isinstance(cases, list) and 1 <= len(cases) <= 512, "case limit")
    counts = dict.fromkeys(("passed", "failed", "skipped", "not_run"), 0)
    passed = set()
    for case in cases:
        common.require(case["outcome"] in counts, "unknown case outcome")
        counts[case["outcome"]] += 1
        if case["outcome"] == "passed":
            passed.add(case["name"].rsplit("/", 1)[-1])
    common.require(
        REQUIRED_CASES <= passed and not counts["failed"] and not counts["not_run"],
        "guest installation/validation must actually pass",
    )
    result["unselectedScenarioCount"] = len(rows) - 1
    unselected = dict.fromkeys(("plain", "registry", "uki", "unknown"), 0)
    for other in rows:
        if other["selected"]:
            continue
        config = other["variant"]["tridentConfigurationName"]
        kind = (
            "registry"
            if config == "extensions"
            else "uki" if config in UKI else "plain" if config in PLAIN else "unknown"
        )
        unselected[kind] += 1
    result["unselectedConfigurationCounts"] = unselected
    result["caseCounts"] = counts


def log_markers(path):
    try:
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        with os.fdopen(fd, "rb") as stream:
            info = os.fstat(stream.fileno())
            common.require(stat.S_ISREG(info.st_mode), "regular log required")
            sample = stream.read(32768)
            if info.st_size > 32768:
                stream.seek(max(32768, info.st_size - 32768))
                sample += b"\n" + stream.read(32768)
        lowered = sample.lower()
        return {
            "sample": "head-and-tail" if info.st_size > 32768 else "head",
            "markers": [
                name
                for name, patterns in LOG_MARKERS.items()
                if any(pattern in lowered for pattern in patterns)
            ],
        }
    except (OSError, ValueError):
        return {"sample": "unavailable", "markers": []}


def install_failure(directory):
    fact = {
        "state": "unavailable",
        "basis": "bound-install-os-junit-message-not-independent-root-cause",
        "operation": "unclassified",
        "knownCommand": None,
        "commandExitCode": None,
        "missingKnownPaths": [],
    }
    try:
        raw = read_bounded(directory / SCENARIO / "storm-e2e.junit.xml", 1024 * 1024)
        fact["state"] = "invalid"
        common.require(
            b"\0" not in raw and b"<!DOCTYPE" not in raw and b"<!ENTITY" not in raw,
            "plain UTF-8 JUnit required",
        )
        root = ET.fromstring(raw.decode("utf-8"))
        suites = root.findall("testsuite")
        common.require(
            root.tag == "testsuites"
            and len(suites) == 1
            and suites[0].get("name") == SCENARIO,
            "JUnit scenario mismatch",
        )
        cases = list(root.iter("testcase"))
        common.require(len(cases) <= 512, "JUnit case limit")
        selected = [case for case in cases if case.get("name") == "install-os"]
        common.require(
            len(selected) == 1
            and selected[0].get("classname") == SCENARIO
            and selected[0].get("status") in ("FAIL", "ERRO"),
            "JUnit failed install case required",
        )
        errors = selected[0].findall("failure") + selected[0].findall("error")
        common.require(len(errors) == 1, "one install failure required")
        message = errors[0].get("message")
        common.require(isinstance(message, str), "JUnit failure message required")
        prefixes = INSTALL_OPERATIONS
        uefi = "failed to set UEFI variables: "
        if message.startswith(uefi):
            message = message[len(uefi) :]
            prefixes = UEFI_OPERATIONS
            fact["operation"] = "uefi-unclassified"
        fact["operation"] = next(
            (name for name, prefix in prefixes.items() if message.startswith(prefix)),
            fact["operation"],
        )
        if fact["operation"] == "uefi-virt-fw-vars":
            fact["knownCommand"] = "sudo-virt-fw-vars"
            match = re.fullmatch(
                r"failed to set boot URI: exit status ([0-9]{1,3})", message
            )
            if match and 1 <= int(match[1]) <= 255:
                fact["commandExitCode"] = int(match[1])
        fact["missingKnownPaths"] = [
            path
            for path in INSTALL_MISSING_PATHS
            if re.search(
                r"""(?<![\w/.-])['"]?"""
                + re.escape(path)
                + r"""['"]?: [Nn]o such file or directory(?:$|\n)""",
                message,
            )
        ]
        fact["state"] = "captured"
    except OSError:
        fact["state"] = "unavailable"
    except (ValueError, ET.ParseError):
        fact["state"] = "invalid"
    return fact


def diagnose_execution(directory, process_status, result):
    diagnostic = {
        "basis": "bounded-runner-facts-and-log-markers-not-root-cause",
        "runnerProcessExitCode": (
            process_status
            if type(process_status) is int and -128 <= process_status <= 255
            else None
        ),
        "resultState": "unavailable",
        "runnerFailureKind": "unclassified",
        "selectedOutcome": "unknown",
        "failedCaseIds": [],
        "unrecognizedCaseCount": 0,
        "installOsFailure": None,
        "logs": {
            name: log_markers(path)
            for name, path in (
                ("runner", directory.parent / "execution.log"),
                ("matrix", directory / "matrix.log"),
                ("scenario", directory / SCENARIO / "console.log"),
            )
        },
    }
    result["executionDiagnostic"] = diagnostic
    try:
        _, data = read_json(directory / "results.json", 1024 * 1024)
        diagnostic["resultState"] = "invalid"
        common.require(
            data["suite"] == "storm-vm-host-install-configuration"
            and data["candidateRevision"] == SOURCE
            and data["manifestSha256"] == result["manifestSha256"]
            and data["stageType"] == STAGE,
            "diagnostic binding mismatch",
        )
        rows = data["scenarios"]
        common.require(isinstance(rows, list) and len(rows) <= 64, "matrix limit")
        selected = [row for row in rows if row["selected"] is True]
        common.require(
            not rows or (len(selected) == 1 and selected[0]["scenario"] == SCENARIO),
            "diagnostic selection mismatch",
        )
        if selected:
            row = selected[0]
            common.require(
                row["outcome"] in ("passed", "failed", "not_run"),
                "unknown selected outcome",
            )
            cases = row["cases"]
            common.require(isinstance(cases, list) and len(cases) <= 512, "case limit")
            counts = dict.fromkeys(("passed", "failed", "skipped", "not_run"), 0)
            failed = set()
            unknown = 0
            for case in cases:
                common.require(
                    case["outcome"] in counts
                    and isinstance(case["name"], str)
                    and len(case["name"]) <= 256,
                    "invalid diagnostic case",
                )
                counts[case["outcome"]] += 1
                name = case["name"].rsplit("/", 1)[-1]
                if name not in DIAGNOSTIC_CASES:
                    unknown += 1
                elif case["outcome"] == "failed":
                    failed.add(name)
            diagnostic["selectedOutcome"] = row["outcome"]
            diagnostic["failedCaseIds"] = sorted(failed)
            diagnostic["unrecognizedCaseCount"] = unknown
            result["caseCounts"] = counts
        error = data.get("error")
        diagnostic["runnerFailureKind"] = next(
            (
                name
                for name, prefix in RUNNER_FAILURES.items()
                if isinstance(error, str) and error.startswith(prefix)
            ),
            "unclassified",
        )
        diagnostic["resultState"] = "captured"
        if "install-os" in diagnostic["failedCaseIds"]:
            diagnostic["installOsFailure"] = install_failure(directory)
    except OSError:
        diagnostic["resultState"] = "unavailable"
    except (ValueError, KeyError, TypeError):
        diagnostic["resultState"] = "invalid"


class ProcessFailure(Exception):
    pass


def run(argv, work, name, timeout, *, group=False, grace=120, cwd=None):
    return common.command(
        argv, cwd or work, work / name, timeout, grace, group_cleanup=group
    )


def execute(args, output):
    result = report()
    status = 1
    before = None
    work = None
    execution_directory = None
    execution_status = None
    handlers = {
        sig: signal.signal(sig, common.interrupt)
        for sig in (signal.SIGINT, signal.SIGTERM)
    }
    try:
        result["phase"] = "preflight"
        common.require(
            sys.platform == "linux"
            and os.geteuid() == 0
            and os.environ.get("EXCLUSIVE_DISPOSABLE_1ES") == "true",
            "exclusive disposable Linux root context required",
        )
        common.require(
            all(shutil.which(name) for name in COMMANDS), "missing dependency"
        )
        common.require(
            not any(os.environ.get(name) for name in ALTERNATE_CREDENTIALS),
            "alternate credentials forbidden",
        )
        common.require(
            re.fullmatch(r"[0-9a-f]{40}", args.expected_harness_revision)
            and args.expected_harness_revision != SOURCE,
            "distinct exact harness revision required",
        )
        revision = inspection.command(["git", "rev-parse", "HEAD"], timeout=30).strip()
        common.require(revision == args.expected_harness_revision, "harness mismatch")
        result["harnessRevision"] = revision
        # Refuse before any installs. setup-functional also checks this mount.
        inspection.command(["mountpoint", "--quiet", "/mnt/storage"], timeout=30)
        work = Path(tempfile.mkdtemp(prefix="trident-install-", dir="/mnt/storage"))
        private_packet = work / "reviewed-packet.json"
        result["phase"] = "packet"
        packet(args, private_packet)
        result["packetSha256"] = args.evidence_sha256
        result["phase"] = "identity"
        inspection.command(
            [
                "az",
                "extension",
                "show",
                "--name",
                "azure-devops",
                "--query",
                "path",
                "-o",
                "tsv",
            ],
            timeout=30,
        )
        inspection.command(
            [
                "az",
                "login",
                "--identity",
                "--client-id",
                "e2fd61e6-b160-4f02-a5ac-3d949a4fe01d",
                "--allow-no-subscriptions",
                "--output",
                "none",
            ],
            timeout=120,
        )
        result["phase"] = "prepare"
        scripts = Path(__file__).resolve().parent
        prepared = work / "prepared"
        # Keep credential acquisition and full server/packet qualification inside
        # the existing preparer's bounded process group, before any installs.
        status = run(
            [
                sys.executable,
                str(scripts / "prepare_vm_e2e_artifacts.py"),
                "--build-id",
                str(inspection.BUILD),
                "--expected-definition-id",
                "5073",
                "--expected-revision",
                SOURCE,
                "--expected-ref",
                inspection.REF,
                "--evidence",
                str(private_packet),
                "--evidence-sha256",
                args.evidence_sha256,
                "--timeout-seconds",
                "600",
                "--output",
                str(prepared),
            ],
            work,
            "prepare.log",
            1200,
            group=True,
        )
        if status:
            raise ProcessFailure()
        bundle = prepared / "bundle"
        manifest_path = bundle / "provenance.json"
        pin = common.digest(manifest_path)
        manifest = common.verify_bundle(bundle, manifest_path, pin, SOURCE)
        common.require(
            manifest["producer"] == prepare.producer(producer_args(private_packet))
            and manifest["lineage"]["packetSha256"] == args.evidence_sha256
            and manifest["lineage"]["basis"]
            == "independently-reviewed-pinned-evidence",
            "prepared qualification mismatch",
        )
        result["manifestSha256"] = pin
        result["phase"] = "host-setup"
        # Approved existing local host setup, private logs, no source startup scope.
        status = run(
            ["bash", str(scripts / "setup-functional.sh")],
            work,
            "host-setup.log",
            900,
            group=True,
            cwd=scripts.parents[1],
        )
        if status:
            raise ProcessFailure()
        with common.host_lock():
            common.check_host()
            before = inventory()
            common.require(not before["domains"], "host must have no existing guests")
            # Guest disks/NVRAM live in virtdeploy's /var/lib/libvirt pools, not
            # in root-private evidence. Check QEMU ancestor search without chmod.
            for directory in ("/var", "/var/lib", "/var/lib/libvirt"):
                inspection.command(
                    ["runuser", "-u", "libvirt-qemu", "--", "test", "-x", directory],
                    timeout=30,
                )
        result["phase"] = "execution"
        execution = work / "execution"
        execution_directory = execution
        status = execution_status = run(
            [
                sys.executable,
                str(scripts / "run-vm-e2e-install.py"),
                "--bundle",
                str(bundle),
                "--manifest",
                str(manifest_path),
                "--manifest-sha256",
                pin,
                "--expected-revision",
                SOURCE,
                "--stage-type",
                STAGE,
                "--scenario",
                SCENARIO,
                "--timeout-seconds",
                "1800",
                "--cleanup-grace-seconds",
                "120",
                "--output",
                str(execution),
            ],
            work,
            "execution.log",
            3780,
            grace=180,
        )
        if status:
            raise ProcessFailure()
        summarize(execution / "results.json", result)
        common.verify_bundle(bundle, manifest_path, pin, SOURCE)
        result["outcome"] = "selected-scenario-passed"
    except KeyboardInterrupt:
        status = 130
    except Exception:
        # Exceptions from tools, XML, credentials or producer APIs are private.
        status = status or 1
    finally:
        with common.cleanup_signals():
            if result["phase"] == "prepare" and work is not None:
                result["preparationDiagnostic"] = preparation_failure(
                    work / "prepare.log"
                )
            if execution_directory is not None:
                diagnose_execution(execution_directory, execution_status, result)
            if before is not None:
                try:
                    with common.host_lock():
                        result["resourceInventoryRestored"] = inventory() == before
                except Exception:
                    result["resourceInventoryRestored"] = False
            if not result["resourceInventoryRestored"]:
                status = status or 1
                result["outcome"] = "failed"
            if status:
                result["outcome"] = (
                    "cancelled"
                    if status == 130
                    else "timeout" if status == 124 else "failed"
                )
            publish(output, result)
        for sig, handler in handlers.items():
            signal.signal(sig, handler)
    return status


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--scratch", type=Path, required=True)
    parser.add_argument("--prepare-publication", action="store_true")
    parser.add_argument("--evidence", type=Path)
    parser.add_argument("--evidence-sha256", default=REVIEWED_PACKET_SHA256)
    parser.add_argument("--expected-harness-revision", default="")
    parser.add_argument("--publication-uid", type=int)
    parser.add_argument("--publication-gid", type=int)
    args = parser.parse_args()
    os.umask(0o077)
    if args.prepare_publication:
        inspection.prepare_publication(
            args.output,
            publication_name="execution.json",
            failure_fn=failed_publication,
        )
        return 0

    def cancel(signum, frame):
        raise inspection.Cancelled()

    for sig in (signal.SIGINT, signal.SIGTERM):
        signal.signal(sig, cancel)
    try:
        code = inspection.privileged_inspection(
            args,
            publication_name="execution.json",
            failure_fn=failed_publication,
            execute_fn=lambda output, scratch, bootstrap_guestfish: execute(
                args, output
            ),
        )
        # The FD-pinned report is intentionally not reopened here.
        return code
    except (Exception, inspection.Cancelled):
        return 1


if __name__ == "__main__":
    sys.exit(main())
