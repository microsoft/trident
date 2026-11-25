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
REQUIRED_CASES = {
    "setup-test-host",
    "install-os",
    "check-trident-ssh",
    "validate-install",
}
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


def read_json(path, limit):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, "rb") as stream:
        info = os.fstat(stream.fileno())
        common.require(
            stat.S_ISREG(info.st_mode) and info.st_size <= limit, "bounded regular JSON"
        )
        value = stream.read(limit + 1)
    common.require(len(value) <= limit, "JSON limit")
    return value, json.loads(value)


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
        status = run(
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
