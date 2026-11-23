#!/usr/bin/env python3
"""Fixed supported VM lanes on one exclusive disposable 1ES host."""

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
import time

from contextlib import contextmanager
from pathlib import Path
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parents[2]
PRODUCT = "18dd6cd99d50a4268afd936acb1ee4d2b64881ea"
BASE = "928feb75a262344a41639b600d5c6174b1ae4b1b"
FUNCTIONAL_REVISION = "076bc60a1830a84b6006f68f78981e7c10d17de6"
FUNCTIONAL = "scripts/cloud-agent/functional-validation.py"
FAMILIES = ("install", "legacy", "lifecycle", "acl-agent")
GO_EXCLUSION = "TestUefiFallbackScriptMatchesLegacy"
STAGE_TYPE = "full-validation"
BUILD_PURPOSE = "validation"


def load_driver(name):
    spec = importlib.util.spec_from_file_location(
        "aggregate_" + name.replace("-", "_"),
        Path(__file__).with_name(f"run-vm-e2e-{name}.py"),
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


install = load_driver("install")
legacy = load_driver("legacy")
lifecycle = load_driver("lifecycle")
acl = load_driver("acl-agent")
require = install.require


def git(root, *args):
    return subprocess.check_output(
        ["git", "-C", str(root), *args], stderr=subprocess.DEVNULL, timeout=30
    )


def checkout(root, revision, entrypoint):
    require(re.fullmatch(r"[a-f0-9]{40}", revision), "Full checkout SHA required")
    require(git(root, "rev-parse", "HEAD").decode().strip() == revision, "Wrong HEAD")
    require(not git(root, "status", "--porcelain"), "Checkout must be clean")
    path = root / entrypoint
    require(
        path.is_file() and not path.is_symlink(),
        "Required fixed entrypoint unavailable",
    )
    require(
        path.read_bytes() == git(root, "show", f"HEAD:{entrypoint}"),
        "Entrypoint differs from pinned source",
    )


def read_json(path):
    require(path.is_file() and not path.is_symlink(), "Missing private evidence")
    require(path.stat().st_size <= 8 * 1024 * 1024, "Evidence exceeds size bound")
    return json.loads(path.read_text())


def verify_inputs(args):
    checkout(args.source, PRODUCT, "scripts/cloud-agent/run-functional.sh")
    require(
        args.functional_harness_revision == FUNCTIONAL_REVISION,
        "Expected the reviewed functional candidate adapter",
    )
    checkout(args.functional_harness, args.functional_harness_revision, FUNCTIONAL)
    # Pin the unchanged public drivers even when the aggregate is committed later.
    for family in FAMILIES:
        relative = f"scripts/cloud-agent/run-vm-e2e-{family}.py"
        require(
            (ROOT / relative).read_bytes() == git(ROOT, "show", f"{BASE}:{relative}"),
            "Public driver contract changed",
        )
    manifests = {}
    for family in FAMILIES:
        bundle = getattr(args, family.replace("-", "_") + "_bundle")
        manifest_path = getattr(args, family.replace("-", "_") + "_manifest")
        pin = getattr(args, family.replace("-", "_") + "_manifest_sha256")
        manifests[family] = install.verify_bundle(bundle, manifest_path, pin, PRODUCT)
        require(
            manifests[family]["producer"]["definitionId"] == 5073,
            "Wrong product producer",
        )
        expected_build = 1266405 if family in ("install", "legacy") else 1266514
        require(
            manifests[family]["producer"]["buildId"] == expected_build,
            "Wrong qualified image cohort",
        )
        require(
            os.access(bundle / "bin/storm-trident", os.X_OK), "Storm not executable"
        )
    require(
        args.expected_tool_revision == BASE,
        "Lifecycle tool must be built from committed reviewed 928 source",
    )
    lifecycle.verify_lifecycle(args.lifecycle_bundle, manifests["lifecycle"], args)
    acl.verify(
        SimpleNamespace(
            bundle=args.acl_agent_bundle,
            manifest=args.acl_agent_manifest,
            manifest_sha256=args.acl_agent_manifest_sha256,
            expected_revision=PRODUCT,
            ssh_private_key_path=args.ssh_private_key,
        )
    )
    require(
        all(
            manifests["lifecycle"]["producer"][key]
            == manifests["acl-agent"]["producer"][key]
            for key in (
                "organization",
                "projectId",
                "definitionId",
                "buildId",
                "sourceVersion",
            )
        ),
        "ACL and lifecycle SSH/image cohorts differ",
    )
    # The same independently qualified key is passed to both cohort consumers.
    require(not args.ssh_private_key.is_symlink(), "Private key symlink refused")
    info = args.ssh_private_key.stat()
    require(
        stat.S_ISREG(info.st_mode) and info.st_size and not info.st_mode & 0o077,
        "Private key must be a protected regular file",
    )
    inputs = [args.source, args.functional_harness, args.ssh_private_key]
    inputs += [getattr(args, f.replace("-", "_") + "_bundle") for f in FAMILIES]
    for path in inputs:
        require(
            not path.is_relative_to(args.output)
            and not args.output.is_relative_to(path),
            "Output must not overlap any input",
        )
    for family in FAMILIES:
        bundle = getattr(args, family.replace("-", "_") + "_bundle")
        require(
            bundle.stat().st_dev == args.output.parent.stat().st_dev,
            "Bundle/output filesystem mismatch",
        )
        require(
            not args.ssh_private_key.is_relative_to(bundle),
            "Private key must remain outside all bundles",
        )
    return manifests


def discover(args, manifests, work):
    def command(argv, label, cwd=work):
        code = install.command(
            [str(value) for value in argv],
            cwd,
            work / f"{label}.log",
            args.timeout_seconds,
            args.cleanup_grace_seconds,
        )
        require(code == 0, "Aggregate preflight command failed")
        require(
            (work / f"{label}.log").stat().st_size <= 1024 * 1024,
            "Matrix exceeds evidence bound",
        )
        return (work / f"{label}.log").read_text()

    text = command(
        [args.install_bundle / "bin/storm-trident", "script", "e2e-matrix", STAGE_TYPE],
        "install-matrix",
    )
    matrix = install.parse_matrix(text)
    require(len(matrix) <= 128, "Unexpected installation matrix size")
    selected = {}
    accounting = []
    for name, entry in sorted(matrix.items()):
        require(len(name) <= 128, "Installation scenario name too long")
        if entry["tridentConfigurationName"] == "extensions":
            accounting.append(
                {
                    "lane": name,
                    "outcome": "unavailable",
                    "reason": "Existing install driver does not support registry preparation",
                }
            )
            continue
        iso = f"artifacts/iso/{entry['installerISOName']}.iso"
        require(
            manifests["install"]["files"][iso]["artifact"] == entry["installerISOName"],
            "Missing installation ISO attribution",
        )
        selected[name] = entry
    require(selected, "No supported installation scenarios")
    text = command(
        [
            sys.executable,
            args.source / "tests/e2e_tests/helpers/read_target_configurations.py",
            "--configurations",
            args.source / "tests/e2e_tests/target-configurations.yaml",
            "--env",
            "virtualMachine",
            "--runtimeEnv",
            "host",
            "--purpose",
            BUILD_PURPOSE,
            "--matrix-name",
            "matrixConfigurations",
        ],
        "legacy-matrix",
        args.source,
    )
    lines = [
        line[len(legacy.MATRIX_PREFIX) :]
        for line in text.splitlines()
        if line.startswith(legacy.MATRIX_PREFIX)
    ]
    require(len(lines) == 1, "Missing legacy matrix")
    legacy_matrix = json.loads(lines[0])
    require(
        isinstance(legacy_matrix, dict) and 0 < len(legacy_matrix) <= 128,
        "Invalid legacy matrix",
    )
    configurations = []
    for name, variant in sorted(legacy_matrix.items()):
        require(
            re.fullmatch(r"[a-z0-9][a-z0-9-]{0,127}", name)
            and variant == {"configuration": name},
            "Invalid legacy configuration",
        )
        if name in ("misc", "extensions"):
            accounting.append(
                {
                    "lane": "legacy-" + name,
                    "outcome": "unavailable",
                    "reason": "Existing legacy driver does not support registry preparation",
                }
            )
            continue
        config = legacy.load_yaml(
            args.source
            / "tests/e2e_tests/trident_configurations"
            / name
            / "trident-config.yaml"
        )
        legacy.check_inputs(args.legacy_bundle, manifests["legacy"], config, name)
        configurations.append(name)
    require(configurations, "No supported legacy configurations")
    return selected, configurations, accounting


def preflight(args, manifests, work):
    require(
        os.environ.get("EXCLUSIVE_DISPOSABLE_1ES") == "true"
        and os.environ.get("GITHUB_ACTIONS") == "true"
        and args.expected_hostname == socket.gethostname(),
        "Explicit exclusive disposable 1ES identity required",
    )
    for name in ("GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT"):
        require(
            re.fullmatch(r"[0-9]+", os.environ.get(name, "")), "Missing batch identity"
        )
    # This lock is released before any child driver attempts to acquire it.
    with install.host_lock():
        lifecycle.preflight(args, args.lifecycle_bundle, manifests["lifecycle"], work)
        for tool in (
            "az",
            "flock",
            "ip",
            "stat",
            "bash",
            "git",
            "ssh-keygen",
            "swtpm",
            "virt-fw-vars",
            "qemu-img",
        ):
            require(shutil.which(tool), "Missing aggregate prerequisite")
        code = install.command(
            [
                sys.executable,
                "-c",
                "import sys, pytest, fabric, yaml, cryptography, libvirt, netifaces, "
                "docker, bcrypt, jinja2, virt.firmware; "
                "c=libvirt.open(None); "
                "c.getURI() == 'qemu:///system' or sys.exit(1); c.close()",
            ],
            work,
            work / "legacy-dependencies.log",
            args.timeout_seconds,
            args.cleanup_grace_seconds,
        )
        require(code == 0, "Legacy dependencies unavailable")
        config = manifests["acl-agent"]["aclAgent"]
        require(
            acl.docker_call(
                shutil.which("docker"),
                [
                    "image",
                    "inspect",
                    "--format",
                    "{{.Os}}/{{.Architecture}}",
                    config["postgresImage"],
                ],
            )
            == "linux/amd64",
            "Qualified Postgres not locally available",
        )
        acl.check_network(work, config, args)
        for uri in acl.URIS:
            acl.domains(work, uri, args, "preflight")
        with socket.socket() as probe:
            probe.bind(("0.0.0.0", 4000))
        return discover(args, manifests, work)


def lanes(args, scenarios, configurations):
    rows = []

    def add(name, family, extra=()):
        destination = args.output / name
        argv = [
            sys.executable,
            str(Path(__file__).with_name(f"run-vm-e2e-{family}.py")),
        ]
        prefix = family.replace("-", "_")
        for option in ("bundle", "manifest", "manifest-sha256"):
            argv += [
                "--" + option,
                str(getattr(args, prefix + "_" + option.replace("-", "_"))),
            ]
        argv += [
            "--expected-revision",
            PRODUCT,
            "--output",
            str(destination),
            "--timeout-seconds",
            str(args.timeout_seconds),
            "--cleanup-grace-seconds",
            str(args.cleanup_grace_seconds),
            *extra,
        ]
        rows.append(
            {
                "lane": name,
                "family": family,
                "argv": argv,
                "output": destination,
                "outcome": "not_run",
            }
        )

    rows.append(
        {
            "lane": "functional-smoke-full",
            "family": "functional",
            "argv": [
                sys.executable,
                str(args.functional_harness / FUNCTIONAL),
                str(args.source),
                str(args.output / "functional-smoke-full"),
                "--expected-source-revision",
                PRODUCT,
            ],
            "output": args.output / "functional-smoke-full",
            "outcome": "not_run",
        }
    )
    for name in sorted(scenarios):
        add(
            "install-" + name,
            "install",
            ["--stage-type", STAGE_TYPE, "--scenario", name],
        )
    add(
        "legacy-vm-host",
        "legacy",
        [
            "--source",
            str(args.source),
            "--suite",
            "legacy-vm-host-install-ab",
            "--build-purpose",
            BUILD_PURPOSE,
            *[value for name in configurations for value in ("--configuration", name)],
        ],
    )
    add(
        "qemu-servicing-rollback",
        "lifecycle",
        [
            "--ssh-private-key",
            str(args.ssh_private_key),
            "--expected-tool-revision",
            args.expected_tool_revision,
            "--tool-sha256",
            args.tool_sha256,
            "--expected-hostname",
            args.expected_hostname,
            "--exclusive-disposable-host",
            "--qemu-user",
            args.qemu_user,
        ],
    )
    add(
        "acl-agent",
        "acl-agent",
        [
            "--ssh-private-key-path",
            str(args.ssh_private_key),
        ],
    )
    return rows


@contextmanager
def cancellation(interrupt=False):
    state = {"signal": None}

    def received(signum, frame):
        if state["signal"] is None:
            state["signal"] = signum
            if interrupt:
                raise KeyboardInterrupt

    previous = {
        sig: signal.signal(sig, received) for sig in (signal.SIGINT, signal.SIGTERM)
    }
    try:
        yield state
    finally:
        for sig, handler in previous.items():
            signal.signal(sig, handler)


def supervise(argv, cwd, env, timeout, grace, cancel, group=False, umask=0o077):
    process = subprocess.Popen(
        argv,
        cwd=cwd,
        env=env,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        start_new_session=True,
        umask=umask,
    )
    started = time.monotonic()
    deadline = None
    primary = None
    forced = False
    while True:
        code = process.poll()
        if code is not None:
            break
        now = time.monotonic()
        if primary is None and (cancel["signal"] or now - started >= timeout):
            primary = 128 + cancel["signal"] if cancel["signal"] else 124
            deadline = now + grace
            # Signal only the driver. Its nested sessions own suite cleanup.
            if process.poll() is None:
                try:
                    if group:
                        install.signal_group(process, signal.SIGTERM)
                    else:
                        process.send_signal(signal.SIGTERM)
                except ProcessLookupError:
                    pass
        if deadline is not None and now >= deadline:
            install.signal_group(process, signal.SIGKILL)
            forced = True
            code = process.wait()
            break
        time.sleep(0.05)
    try:
        os.killpg(process.pid, 0)
    except ProcessLookupError:
        pass
    else:
        install.signal_group(process, signal.SIGKILL)
        forced = True
    return {
        "exitCode": (
            primary if primary is not None else (code if code >= 0 else 128 - code)
        ),
        "driverExitCode": code,
        "forced": forced,
    }


def cleanup_uncertain(value):
    if isinstance(value, dict):
        return any(
            (
                key
                in (
                    "requiresHostDisposal",
                    "cleanupErrors",
                    "cleanupError",
                    "listenerCleanupError",
                    "containerCleanupError",
                )
                and bool(item)
            )
            or (key in ("cleanupExitCode", "cleanupCheckExitCode") and item != 0)
            or cleanup_uncertain(item)
            for key, item in value.items()
        )
    if isinstance(value, list):
        return any(cleanup_uncertain(item) for item in value)
    return False


def evidence(row, args):
    if row["family"] == "functional":
        summary = read_json(row["output"] / "summary.json")
        identity = read_json(row["output"] / "identity.json")
        require(
            summary["passed"] is True
            and not summary["not_run"]
            and identity["candidate"] == PRODUCT
            and identity["harness"] == args.functional_harness_revision,
            "Functional adapter did not pass for this candidate",
        )
        counts = {}
        binaries = []
        for mode in ("smoke", "full"):
            result = summary["results"][mode]
            require(
                result["exit"] == 0
                and result["junit"]["tests"] > 0
                and result["junit"]["rust_passed"] > 0
                and result["junit"]["skipped"] == 0,
                "Incomplete fresh functional coverage",
            )
            counts[mode] = {
                name: result["junit"][name]
                for name in ("tests", "rust_passed", "skipped")
            }
            require(
                all(type(value) is int for value in counts[mode].values()),
                "Invalid functional case counts",
            )
            report = read_json(row["output"] / f"{mode}-binaries.json")
            require(
                report["candidate"] == PRODUCT and report["binaries"],
                "Missing candidate binary evidence",
            )
            binaries.append(report["binaries"])
        require(binaries[0] == binaries[1], "Functional binaries changed")
        excluded = read_json(row["output"] / "go-exclusions.json")
        require(
            excluded["test"] == GO_EXCLUSION
            and excluded["full_go_suite_validated"] is False,
            "Unexpected Go exclusion contract",
        )
        return False, counts
    report = read_json(row["output"] / "results.json")
    prefix = row["family"].replace("-", "_")
    require(
        report["candidateRevision"] == PRODUCT
        and report["manifestSha256"] == getattr(args, prefix + "_manifest_sha256"),
        "Driver evidence identity mismatch",
    )
    uncertain = cleanup_uncertain(report)
    if row["exitCode"] == 0:
        require(
            report["exitCode"] == 0 and report["outcome"] == "passed",
            "Driver did not report success",
        )
    return uncertain, {}


def run_lanes(rows, args, cancel, save):
    env = dict(
        os.environ,
        GITHUB_SHA=args.functional_harness_revision,
        LIBVIRT_DEFAULT_URI="qemu:///system",
        LC_ALL="C",
        TF_BUILD="false",
    )
    for row in rows:
        if cancel["signal"]:
            return 128 + cancel["signal"]
        row["outcome"] = "running"
        save()
        try:
            row.update(
                supervise(
                    row["argv"],
                    args.source,
                    env,
                    args.lane_timeout_seconds,
                    args.shutdown_grace_seconds,
                    cancel,
                    group=row["family"] == "functional",
                    # ACL's unchanged mkdir calls need search, not evidence reads.
                    umask=0o066 if row["family"] == "acl-agent" else 0o077,
                )
            )
            if cancel["signal"]:
                row["exitCode"] = row["exitCode"] or 128 + cancel["signal"]
            uncertain, counts = evidence(row, args)
            row["requiresHostDisposal"] = uncertain or row["forced"]
            if counts:
                row["functionalCases"] = counts
            if row["requiresHostDisposal"]:
                row["exitCode"] = row["exitCode"] or 1
        except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError):
            row["exitCode"] = row.get("exitCode") or 1
            row["requiresHostDisposal"] = True
            row["evidenceError"] = True
        if row["exitCode"]:
            # Older drivers cannot always certify cleanup after interruption.
            row["outcome"] = "failed" if "driverExitCode" in row else "unavailable"
            row["requiresHostDisposal"] = True
            save()
            return row["exitCode"]
        row["outcome"] = "passed"
        save()
    return 128 + cancel["signal"] if cancel["signal"] else 0


ACCOUNTING = [
    {"lane": name, "outcome": "excluded", "reason": reason}
    for name, reason in (
        ("arm64", "Requires an ARM64 host or separately qualified emulation"),
        ("bare-metal", "Requires physical hardware and lab services"),
        ("azure-resources", "External-resource provisioning is not approved"),
        (
            "vm-container",
            "Current Storm pipeline include is disabled; not supported here",
        ),
        (
            "scale-direct-streaming",
            "Separate pipeline/configuration, outside fixed local lanes",
        ),
        (
            "rollback-qemu-grub-without-verity",
            "Existing caller sets includeQemuGrub=false",
        ),
        (
            "legacy-pipeline-extras",
            "Existing driver excludes auto/manual rollback interlude, "
            "independent staged/finalized servicing, rebuild-raid and publication",
        ),
    )
] + [
    {
        "lane": "secure-boot-qualification",
        "outcome": "unavailable",
        "reason": "Unsigned UKI cohort is not Secure Boot qualification",
    }
]


def execute(args):
    rows = [
        {"lane": name, "outcome": "not_run"}
        for name in (
            "functional-smoke-full",
            "install-matrix",
            "legacy-vm-host",
            "qemu-servicing-rollback",
            "acl-agent",
        )
    ]
    preflight_cancel = {"signal": None}
    summary = {
        "candidateRevision": PRODUCT,
        "functionalHarnessRevision": (
            args.functional_harness_revision
            if re.fullmatch(r"[a-f0-9]{40}", args.functional_harness_revision)
            else None
        ),
        "outcome": "unavailable",
        "exitCode": 1,
        "firstFailingLane": None,
        "requiresHostDisposal": False,
        "coverage": list(ACCOUNTING),
        "fullGoSuiteValidated": False,
        "goExcludedTest": GO_EXCLUSION,
        "allTridentE2E": False,
        "supportedVmAggregatePassed": False,
        "fullAggregateOK": False,
    }

    def save():
        summary["lanes"] = [
            {
                key: value
                for key, value in row.items()
                if key not in ("argv", "output", "family")
            }
            for row in rows
        ]
        summary["firstFailingLane"] = next(
            (r["lane"] for r in rows if r.get("exitCode")), None
        )
        summary["requiresHostDisposal"] = any(
            r.get("requiresHostDisposal", False) or r["outcome"] == "running"
            for r in rows
        )
        text = json.dumps(summary, indent=2) + "\n"
        require(len(text) <= 1024 * 1024, "Summary exceeds evidence bound")
        (args.output / "summary.json").write_text(text)

    require(not args.output.exists(), "Output must be new")
    require(args.output.parent.is_dir(), "Output parent must already exist")
    with lifecycle.private_creation():
        args.output.mkdir(mode=0o700)
        try:
            save()
            require(
                "system.posix_acl_default" not in os.listxattr(args.output),
                "Output inherited a default ACL",
            )
            manifests = verify_inputs(args)
            summary["manifestSha256"] = {
                family: getattr(args, family.replace("-", "_") + "_manifest_sha256")
                for family in FAMILIES
            }
            summary["toolRevision"] = args.expected_tool_revision
            summary["toolSha256"] = args.tool_sha256
            work = args.output / "preflight"
            work.mkdir(mode=0o700)
            # Only these newly owned directories receive QEMU search access.
            import pwd

            identity = pwd.getpwnam(args.qemu_user)
            require(
                identity.pw_uid > 0 and identity.pw_gid > 0,
                "QEMU identity must be nonroot",
            )
            lifecycle.qemu_searchable(args.output, identity)
            with cancellation(interrupt=True) as preflight_cancel:
                scenarios, configurations, accounting = preflight(args, manifests, work)
            if preflight_cancel["signal"]:
                raise KeyboardInterrupt
            summary["coverage"] += accounting
            rows = lanes(args, scenarios, configurations)
            summary["outcome"] = "running"
            save()
            with cancellation() as cancel:
                code = run_lanes(rows, args, cancel, save)
            summary["supportedVmAggregatePassed"] = code == 0
            summary["fullAggregateOK"] = code == 0 and not accounting
            summary["exitCode"] = code or (0 if summary["fullAggregateOK"] else 1)
            summary["outcome"] = (
                "failed"
                if code
                else ("passed" if summary["fullAggregateOK"] else "incomplete")
            )
        except KeyboardInterrupt:
            summary["exitCode"] = (
                128 + preflight_cancel["signal"] if preflight_cancel["signal"] else 130
            )
        except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError):
            summary["preflightError"] = True
            if preflight_cancel["signal"]:
                summary["exitCode"] = 128 + preflight_cancel["signal"]
        finally:
            if summary["outcome"] == "unavailable":
                for row in rows:
                    row["outcome"] = "unavailable"
            save()
    return summary["exitCode"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("source", "functional-harness", "output", "ssh-private-key"):
        parser.add_argument("--" + name, type=Path, required=True)
    for family in FAMILIES:
        for option in ("bundle", "manifest"):
            parser.add_argument(f"--{family}-{option}", type=Path, required=True)
        parser.add_argument(f"--{family}-manifest-sha256", required=True)
    for name in (
        "functional-harness-revision",
        "expected-hostname",
        "expected-tool-revision",
        "tool-sha256",
    ):
        parser.add_argument("--" + name, required=True)
    parser.add_argument(
        "--exclusive-disposable-host", action="store_true", required=True
    )
    parser.add_argument("--qemu-user", default="libvirt-qemu")
    for name in (
        "timeout-seconds",
        "cleanup-grace-seconds",
        "lane-timeout-seconds",
        "shutdown-grace-seconds",
    ):
        parser.add_argument("--" + name, type=install.positive_seconds, required=True)
    args = parser.parse_args()
    args.expected_revision = PRODUCT
    args.lane = None
    for name, value in vars(args).items():
        if isinstance(value, Path):
            # Retain the private-key leaf for the no-symlink check.
            setattr(
                args,
                name,
                value.absolute() if name == "ssh_private_key" else value.resolve(),
            )
    signal.signal(signal.SIGTERM, install.interrupt)
    try:
        return execute(args)
    except (OSError, ValueError):
        print(
            "Aggregate unavailable; no runtime qualification claimed", file=sys.stderr
        )
        return 1


if __name__ == "__main__":
    sys.exit(main())
