#!/usr/bin/env python3

import argparse
import importlib.util
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import uuid
import xml.etree.ElementTree as ET

from pathlib import Path

SPEC = importlib.util.spec_from_file_location(
    "vm_install", Path(__file__).with_name("run-vm-e2e-install.py")
)
common = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(common)
MATRIX_PREFIX = "##vso[task.setvariable variable=matrixConfigurations;isOutput=true]"
PHASES = ("clean-install", "pytest-a", "update-b", "pytest-b", "update-a", "pytest-a2")
CLEANUP_CHECK = """
import libvirt
import sys
prefix = sys.argv[1]
connection = libvirt.open(None)
for lookup, suffix, missing in (
    (connection.lookupByName, "-vm-0", libvirt.VIR_ERR_NO_DOMAIN),
    (connection.storagePoolLookupByName, "-pool", libvirt.VIR_ERR_NO_STORAGE_POOL),
    (connection.networkLookupByName, "-network", libvirt.VIR_ERR_NO_NETWORK),
):
    try:
        lookup(prefix + suffix)
    except libvirt.libvirtError as error:
        if error.get_error_code() != missing:
            raise
    else:
        raise RuntimeError("Cleanup left owned resource: " + prefix + suffix)
connection.close()
"""


class PhaseFailure(Exception):
    def __init__(self, status):
        self.status = status


def pytest_results(path):
    cases = []
    for case in ET.parse(path).getroot().iter("testcase"):
        common.require(case.get("name"), "pytest reported an unnamed test case")
        outcome = "passed"
        if case.find("failure") is not None or case.find("error") is not None:
            outcome = "failed"
        elif case.find("skipped") is not None:
            outcome = "skipped"
        cases.append(
            {
                "name": f"{case.get('classname', '')}::{case.attrib['name']}",
                "outcome": outcome,
            }
        )
    common.require(cases, "pytest produced no executed test report")
    return cases


def run_command(args, argv, cwd, log):
    return common.command(
        [str(arg) for arg in argv],
        cwd,
        log,
        args.timeout_seconds,
        args.cleanup_grace_seconds,
        group_cleanup=True,
    )


def load_yaml(path):
    import yaml

    try:
        return yaml.safe_load(path.read_text())
    except yaml.YAMLError as error:
        raise ValueError(f"Invalid legacy configuration: {path}") from error


def check_inputs(bundle, manifest, config, name):
    common.require(
        name not in ("misc", "extensions"),
        f"{name}: registry-backed configuration requires candidate OCI preparation "
        "and authentication, unavailable in this lane",
    )
    image = config["image"]["url"]
    match = re.fullmatch(
        r"http://NETLAUNCH_HOST_ADDRESS/files/(regular|verity|usrverity)\.cosi", image
    )
    common.require(match, f"Unsupported legacy image URL: {image}")
    kind = match[1]
    artifact = {
        "regular": "trident-testimage",
        "verity": "trident-verity-testimage",
        "usrverity": "trident-usrverity-testimage",
    }[kind]
    installer = "trident-split-installer" if name == "split" else "trident-installer"
    needed = {f"artifacts/iso/{installer}.iso": installer}
    ab = config["storage"].get("abUpdate") is not None
    versions = ("", "_v2", "_v3") if ab else ("",)
    needed.update({f"artifacts/test-image/{kind}{v}.cosi": artifact for v in versions})
    for tool in ("storm-trident", "netlaunch", "netlisten"):
        needed[f"bin/{tool}"] = "go-tools"
        common.require(
            os.access(bundle / f"bin/{tool}", os.X_OK), f"Missing executable: {tool}"
        )
    for path, source in needed.items():
        common.require(
            path in manifest["files"] and manifest["files"][path]["artifact"] == source,
            f"Missing candidate artifact: {path} (from {source})",
        )
    if kind == "usrverity":
        common.require(
            manifest.get("signingCertificate"), "Missing UKI signing certificate"
        )
        common.require(
            manifest["files"][manifest["signingCertificate"]]["artifact"]
            == "trident-usrverity-testimage",
            "UKI certificate must come from trident-usrverity-testimage",
        )
    return installer, ab


def run_suite(args, bundle, manifest, source, output, row):
    row["outcome"] = "failed"
    with tempfile.TemporaryDirectory(
        prefix="legacy-vm-", dir=output.parent.parent
    ) as temp:
        work = Path(temp)
        tests = work / "tests/e2e_tests"
        shutil.copytree(
            source / "tests/e2e_tests",
            tests,
            ignore=shutil.ignore_patterns("key", "__pycache__", "ts.json"),
        )
        shutil.copytree(source / "tools/virtdeploy", work / "tools/virtdeploy")
        shutil.copy2(source / "tools/virt-deploy", work / "tools/virt-deploy")
        images = work / "artifacts/test-image"
        shutil.copytree(bundle / "artifacts/test-image", images, copy_function=os.link)
        config_dir = tests / "trident_configurations" / row["configuration"]
        config = config_dir / "trident-config.yaml"
        key = tests / "helpers/key"
        storm = bundle / "bin/storm-trident"
        prefix = "legacy-" + uuid.uuid4().hex + "-"
        virt = [
            sys.executable,
            work / "tools/virt-deploy",
            "--nameprefix",
            prefix,
            "create",
        ]
        metadata = work / "tools/virt-deploy-metadata.json"
        netconfig = work / "tools/vm-netlaunch.yaml"
        listener = None
        listener_log = None
        created = False

        def step(label, argv, cwd=work):
            status = run_command(args, argv, cwd, output / f"{label}.log")
            row.setdefault("steps", []).append({"name": label, "exitCode": status})
            if status:
                raise PhaseFailure(status)

        def ssh_helper(name, *extra):
            return [storm, "helper", name, key, host, "testing-user", "host", *extra]

        try:
            step(
                "prepare-config",
                [
                    sys.executable,
                    tests / "helpers/edit_host_config.py",
                    "--keypath",
                    key,
                    "--hostconfig",
                    config,
                    "--runtimeEnv",
                    "host",
                ],
            )
            key.chmod(0o600)
            step("key-pem", ["ssh-keygen", "-p", "-m", "PEM", "-f", key, "-N", ""])
            if row["configuration"] != "rerun":
                step(
                    "uefi-fallback",
                    [
                        sys.executable,
                        tests / "helpers/inject_uefi_fallback_validation.py",
                        "--hostconfig",
                        config,
                        "--runtimeEnv",
                        "host",
                    ],
                )
            created = True  # Partial creation also requires namespace-local cleanup.
            step("create", [*virt, "--mem", "12", "--disks", "32,32"])
            host = json.loads(metadata.read_text())["virtualmachines"][0]["ip"]
            argv = [
                bundle / "bin/netlaunch",
                "--iso",
                bundle / f"artifacts/iso/{row['installer']}.iso",
                "--config",
                netconfig,
                "--trident",
                config,
                "--servefolder",
                images,
                "--logstream",
                "--trace-file",
                output / "install-metrics.jsonl",
                "--force-color",
                "--full-logstream",
                output / "logstream-full.log",
                "--secure-boot",
                "--port",
                "4000",
            ]
            health = row["configuration"] == "health-checks-install"
            argv += [
                "--only-print-exit-code" if health else "--wait-for-provisioned-state"
            ]
            if row["configuration"] in ("rerun", "memory-constraint-combined"):
                argv += ["--max-failures", "1"]
            if manifest.get("signingCertificate"):
                argv += [
                    "--signing-cert",
                    common.bundle_path(bundle, manifest["signingCertificate"]),
                ]
            for phase in row["phases"]:
                if phase["outcome"] == "skipped":
                    continue
                phase["executed"] = True
                phase["outcome"] = "failed"
                if phase["name"] == "clean-install":
                    step("netlaunch", argv)
                    step(
                        "check-ssh",
                        ssh_helper(
                            "check-ssh", *(["--expect-failed-commit"] if health else [])
                        ),
                    )
                    step(
                        "check-tracing",
                        ssh_helper(
                            "check-tracing",
                            "--metric-file",
                            output / "install-metrics.jsonl",
                        ),
                    )
                    step(
                        "check-selinux",
                        ssh_helper(
                            "check-selinux", "--audit-file", output / "audit.log"
                        ),
                    )
                elif phase["name"].startswith("update"):
                    listener_log = (output / f"{phase['name']}-netlisten.log").open(
                        "wb"
                    )
                    listener = subprocess.Popen(
                        [
                            str(bundle / "bin/netlisten"),
                            "--force-color",
                            "--full-logstream",
                            str(output / f"{phase['name']}-logstream.log"),
                            "-m",
                            str(output / f"{phase['name']}-metrics.jsonl"),
                            "-s",
                            str(images),
                            "-p",
                            "4000",
                        ],
                        cwd=work,
                        stdout=listener_log,
                        stderr=subprocess.STDOUT,
                        start_new_session=True,
                    )
                    common.require(
                        listener.poll() is None, "netlisten exited before A/B update"
                    )
                    step(
                        phase["name"],
                        ssh_helper(
                            "ab-update",
                            "--trident-config",
                            "/var/lib/trident/config.yaml",
                            "--version",
                            str(phase["version"]),
                            "--stage-ab-update",
                            "--finalize-ab-update",
                        ),
                    )
                    try:
                        listener_status = listener.wait(timeout=args.timeout_seconds)
                    except subprocess.TimeoutExpired:
                        phase["listenerOutcome"] = "timed_out"
                        raise PhaseFailure(124)
                    phase["listenerExitCode"] = listener_status
                    if listener_status:
                        raise PhaseFailure(listener_status)
                    common.reap(listener, args.cleanup_grace_seconds, group=True)
                    listener = None
                    listener_log.close()
                    listener_log = None
                else:
                    junit = output / f"{phase['name']}.junit.xml"
                    cmd = [
                        sys.executable,
                        "-u",
                        "-m",
                        "pytest",
                        "-m",
                        args.build_purpose,
                        "--capture=no",
                        f"--junit-xml={junit}",
                        "--host",
                        host,
                        "--runtime-env",
                        "host",
                        "--configuration",
                        config_dir,
                        "--keypath",
                        key,
                        "--ab-active-volume",
                        phase["expectedActiveVolume"],
                    ]
                    if health:
                        cmd += ["--expected-host-status-state", "not-provisioned"]
                    status = run_command(
                        args, cmd, tests, output / f"{phase['name']}.log"
                    )
                    phase["exitCode"] = status
                    try:
                        phase["cases"] = pytest_results(junit)
                        common.require(
                            any(c["outcome"] == "passed" for c in phase["cases"])
                            and all(c["outcome"] != "failed" for c in phase["cases"]),
                            "pytest returned no passes or reported failures",
                        )
                    except (OSError, ValueError, ET.ParseError) as error:
                        phase["reportError"] = str(error)
                        status = status or 1
                    if status:
                        raise PhaseFailure(status)
                phase["outcome"] = "passed"
                phase["activeVolumeAfter"] = phase["expectedActiveVolume"]
            row["outcome"] = "passed"
        except PhaseFailure as error:
            row["exitCode"] = error.status
        except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
            row["error"] = str(error)
            row["exitCode"] = 1
        except KeyboardInterrupt:
            row["exitCode"] = 130
        finally:
            # Never clean underneath a suite's phases, or release the lock while reaping.
            with common.cleanup_signals():
                try:
                    if listener is not None:
                        common.reap(listener, args.cleanup_grace_seconds, group=True)
                except (OSError, subprocess.SubprocessError) as error:
                    row["listenerCleanupError"] = str(error)
                    row["exitCode"] = row.get("exitCode") or 1
                    row["outcome"] = "failed"
                finally:
                    if listener_log is not None:
                        listener_log.close()
                if created:
                    try:
                        cleanup = run_command(
                            args, [*virt, "--clean"], work, output / "cleanup.log"
                        )
                        row["cleanupExitCode"] = cleanup
                        verified = run_command(
                            args,
                            [sys.executable, "-c", CLEANUP_CHECK, prefix],
                            work,
                            output / "cleanup-check.log",
                        )
                        row["cleanupCheckExitCode"] = verified
                        cleanup = cleanup or verified
                    except (OSError, subprocess.SubprocessError) as error:
                        row["cleanupError"] = str(error)
                        cleanup = 1
                    if cleanup:
                        row["exitCode"] = row.get("exitCode") or cleanup
                        row["outcome"] = "failed"
        return row.get("exitCode", 0)


def execute(args):
    bundle, output, source = (
        args.bundle.resolve(),
        args.output.resolve(),
        args.source.resolve(),
    )
    common.require(
        not output.is_relative_to(bundle)
        and not bundle.is_relative_to(output)
        and not output.is_relative_to(source)
        and not source.is_relative_to(output),
        "Output, source and bundle must not overlap",
    )
    output.mkdir(parents=True, exist_ok=False)
    result = {
        "suite": args.suite,
        "candidateRevision": args.expected_revision,
        "manifestSha256": args.manifest_sha256,
        "buildPurpose": args.build_purpose,
        "outcome": "unavailable",
        "configurations": [],
    }
    status = 1
    try:
        manifest = common.verify_bundle(
            bundle,
            args.manifest.resolve(),
            args.manifest_sha256,
            args.expected_revision,
        )
        result["provenance"] = manifest
        with common.host_lock():
            common.check_host()
            status = run_command(
                args,
                [
                    sys.executable,
                    "-c",
                    "import sys, pytest, fabric, yaml, cryptography, libvirt, netifaces, "
                    "docker, bcrypt, jinja2, virt.firmware; "
                    "connection = libvirt.open(None); "
                    "connection.getURI() == 'qemu:///system' or "
                    "sys.exit('Wrong default libvirt URI'); "
                    "connection.close()",
                ],
                source,
                output / "dependencies.log",
            )
            common.require(
                status == 0, "Missing legacy Python prerequisites, see dependencies.log"
            )
            for tool in ("ssh-keygen", "swtpm", "virt-fw-vars"):
                common.require(
                    shutil.which(tool), f"Missing legacy prerequisite: {tool}"
                )
            status = run_command(
                args,
                [
                    sys.executable,
                    source / "tests/e2e_tests/helpers/read_target_configurations.py",
                    "--configurations",
                    source / "tests/e2e_tests/target-configurations.yaml",
                    "--env",
                    "virtualMachine",
                    "--runtimeEnv",
                    "host",
                    "--purpose",
                    args.build_purpose,
                    "--matrix-name",
                    "matrixConfigurations",
                ],
                source,
                output / "matrix.log",
            )
            common.require(status == 0, f"Legacy matrix command failed: {status}")
            lines = [
                line[len(MATRIX_PREFIX) :]
                for line in (output / "matrix.log").read_text().splitlines()
                if line.startswith(MATRIX_PREFIX)
            ]
            common.require(len(lines) == 1, "Expected exactly one legacy matrix")
            matrix = json.loads(lines[0])
            common.require(isinstance(matrix, dict) and matrix, "Empty legacy matrix")
            common.require(
                set(args.configuration) <= set(matrix),
                "Requested configuration not in legacy matrix",
            )
            for name, variant in sorted(matrix.items()):
                common.require(
                    re.fullmatch(r"[a-z0-9][a-z0-9-]*", name)
                    and variant == {"configuration": name},
                    "Malformed legacy matrix entry",
                )
                result["configurations"].append(
                    {
                        "configuration": name,
                        "variant": variant,
                        "selected": name in args.configuration,
                        "outcome": "not_run",
                    }
                )
            for row in result["configurations"]:
                if not row["selected"]:
                    continue
                row["outcome"] = "unavailable"
                config = load_yaml(
                    source
                    / "tests/e2e_tests/trident_configurations"
                    / row["configuration"]
                    / "trident-config.yaml"
                )
                row["installer"], ab = check_inputs(
                    bundle, manifest, config, row["configuration"]
                )
                row["phases"] = [
                    {
                        "name": name,
                        "executed": False,
                        "outcome": "not_run" if ab or i < 2 else "skipped",
                        "expectedActiveVolume": (
                            "volume-b" if i in (2, 3) else "volume-a"
                        ),
                        **({"version": 2 if i == 2 else 3} if i in (2, 4) else {}),
                    }
                    for i, name in enumerate(PHASES)
                ]
                row["outcome"] = "not_run"
            result["outcome"] = "failed"
            for row in result["configurations"]:
                if row["selected"]:
                    suite_output = output / row["configuration"]
                    suite_output.mkdir()
                    status = run_suite(
                        args, bundle, manifest, source, suite_output, row
                    )
                    if status:
                        break
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
        status = 130
    finally:
        result["exitCode"] = status
        (output / "results.json").write_text(json.dumps(result, indent=2) + "\n")
    return status


def main():
    parser = argparse.ArgumentParser(
        description="Explicit legacy netlaunch + pytest VM/HOST phases"
    )
    parser.add_argument("--suite", choices=["legacy-vm-host-install-ab"], required=True)
    parser.add_argument(
        "--source", type=Path, default=Path(__file__).resolve().parents[2]
    )
    for name in ("bundle", "manifest", "output"):
        parser.add_argument(f"--{name}", type=Path, required=True)
    parser.add_argument("--manifest-sha256", required=True)
    parser.add_argument("--expected-revision", required=True)
    parser.add_argument(
        "--build-purpose",
        choices=["pullrequest", "post_merge", "daily", "weekly", "validation"],
        required=True,
    )
    parser.add_argument("--configuration", action="append", required=True)
    parser.add_argument(
        "--timeout-seconds", type=common.positive_seconds, required=True
    )
    parser.add_argument(
        "--cleanup-grace-seconds", type=common.positive_seconds, required=True
    )
    args = parser.parse_args()
    signal.signal(signal.SIGTERM, common.interrupt)
    try:
        return execute(args)
    except (OSError, ValueError) as error:
        print(error, file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
