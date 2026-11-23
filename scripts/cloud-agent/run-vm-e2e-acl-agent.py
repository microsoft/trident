#!/usr/bin/env python3

import argparse
import importlib.util
import ipaddress
import json
import os
import re
import shlex
import shutil
import signal
import socket
import subprocess
import sys
import time
import uuid
import xml.etree.ElementTree as ET

from contextlib import contextmanager
from pathlib import Path

SPEC = importlib.util.spec_from_file_location(
    "vm_install", Path(__file__).with_name("run-vm-e2e-install.py")
)
common = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(common)
require = common.require
BASE = "artifacts/acl-agent/trident-vm-acl-agent-testimage.qcow2"
UPDATE = "artifacts/test-image/trident-vm-acl-agent-update-testimage.cosi"
LABEL = "io.trident.vm-e2e-acl-agent"
URIS = ("qemu:///system", "qemu:///session")
CASES = (
    "deploy-vm",
    "check-deployment",
    "run-node-resilience",
    "run-node-name-source",
    "run-ab-update",
    "run-rollback",
    "collect-logs",
    "cleanup-vm",
)


def verify(args):
    require(
        not args.ssh_private_key_path.resolve().is_relative_to(args.bundle),
        "SSH identity must remain external to the artifact bundle",
    )
    manifest = common.verify_bundle(
        args.bundle, args.manifest, args.manifest_sha256, args.expected_revision
    )
    config = manifest["aclAgent"]
    require(
        set(config)
        == {"configRevision", "postgresImage", "targetVersion", "hostEndpointIP"},
        "Expected explicit ACL-agent configuration",
    )
    require(
        config["configRevision"] == args.expected_revision,
        "ACL-agent configuration must match the candidate",
    )
    require(
        re.fullmatch(
            r"[a-z0-9][a-z0-9.:-]*[.:][a-z0-9.:-]+/"
            r"[a-z0-9/_-]+@sha256:[0-9a-f]{64}",
            config["postgresImage"],
        ),
        "Postgres requires a fully qualified registry/repository@sha256 digest",
    )
    require(
        re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", config["targetVersion"]),
        "Explicit update image OS version required",
    )
    address = ipaddress.IPv4Address(config["hostEndpointIP"])
    require(
        address.is_private and not address.is_loopback and not address.is_unspecified,
        "Explicit guest-reachable private host endpoint required",
    )
    for path, artifact in (
        (BASE, "image-acl-agent-base"),
        (UPDATE, "image-acl-agent-update"),
    ):
        require(
            manifest["files"][path]["artifact"] == artifact,
            f"Missing ACL-agent pipeline artifact: {artifact}",
        )
    require(
        os.access(args.bundle / "bin/storm-trident", os.X_OK),
        "Candidate storm-trident is not executable",
    )
    # Only metadata is examined. The supplied key never enters the bundle/output.
    require(args.ssh_private_key_path.is_file(), "SSH key prerequisite unavailable")
    return manifest


def docker_call(docker, argv, timeout=30):
    # Never retain Docker stderr, argv or environment, including test DB settings.
    process = subprocess.run(
        [docker, *argv],
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        timeout=timeout,
        check=False,
    )
    require(process.returncode == 0, "Docker prerequisite/service operation failed")
    return process.stdout.decode().strip()


def owned_container(config, container, timeout=30):
    require(re.fullmatch(r"[0-9a-f]{64}", container), "Invalid container ID")
    owner = docker_call(
        config["docker"],
        ["inspect", "--format", '{{index .Config.Labels "' + LABEL + '"}}', container],
        timeout=timeout,
    )
    require(owner == config["owner"], "Refusing access to an unowned container")


def durable_record(path, value):
    with path.open("x", encoding="utf-8") as stream:
        stream.write(value + "\n")
        stream.flush()
        os.fsync(stream.fileno())
    directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)


def docker_proxy(config, argv):
    require(argv, "Missing Docker operation")
    docker = config["docker"]
    if argv[0] == "run":
        require(
            argv[-1] == config["image"]
            and not any(
                arg.split("=")[0] in ("--name", "--label", "--pull", "--privileged")
                for arg in argv[1:]
            ),
            "Unexpected ACL-agent Postgres command/image",
        )
        name = f"{config['owner']}-{uuid.uuid4().hex}"
        intent = Path(config["starts"]) / f"{name}.intent"
        # A killed CLI cannot cancel an already accepted daemon create request.
        durable_record(intent, name)
        container = docker_call(
            docker,
            [
                "run",
                "--pull=never",
                "--label",
                f"{LABEL}={config['owner']}",
                "--name",
                name,
                "--health-cmd",
                "pg_isready -U postgres -d nebraska",
                "--health-interval",
                "1s",
                "--health-timeout",
                "2s",
                "--health-retries",
                "30",
                *argv[1:],
            ],
        )
        require(re.fullmatch(r"[0-9a-f]{64}", container), "Invalid container ID")
        durable_record(intent.with_suffix(".created"), container)
        owned_container(config, container)
        deadline = time.monotonic() + 40
        while time.monotonic() < deadline:
            health = docker_call(
                docker,
                ["inspect", "--format", "{{.State.Health.Status}}", container],
                timeout=3,
            )
            if health == "healthy":
                return container
            require(health == "starting", "Postgres failed its health check")
            time.sleep(0.5)
        raise ValueError("Postgres health check timed out")
    require(
        (len(argv) == 3 and argv[:2] == ["rm", "-f"])
        or (len(argv) == 3 and argv[0] == "port" and argv[2] == "5432/tcp"),
        "Unsupported ACL-agent Docker operation",
    )
    container = argv[2] if argv[0] == "rm" else argv[1]
    owned_container(config, container)
    return docker_call(docker, argv)


@contextmanager
def service_environment(output, config):
    config["starts"] = str(output / "postgres-starts")
    Path(config["starts"]).mkdir()
    proxy = output / "docker-proxy.json"
    proxy.write_text(json.dumps(config) + "\n")
    tools = output / "bin"
    tools.mkdir()
    wrapper = tools / "docker"
    wrapper.write_text(
        "#!/bin/sh\nexec "
        + " ".join(
            shlex.quote(value)
            for value in (
                sys.executable,
                str(Path(__file__).resolve()),
                "docker-proxy",
                str(proxy),
            )
        )
        + ' "$@"\n'
    )
    wrapper.chmod(0o700)
    previous = {name: os.environ.get(name) for name in ("PATH", "TF_BUILD", "LC_ALL")}
    os.environ["PATH"] = str(tools) + os.pathsep + (previous["PATH"] or "")
    os.environ["TF_BUILD"] = "false"
    os.environ["LC_ALL"] = "C"
    try:
        yield
    finally:
        for name, value in previous.items():
            if value is None:
                del os.environ[name]
            else:
                os.environ[name] = value


def operation(argv, output, name, args):
    log = output / f"{name}.log"
    previous = os.environ.get("LC_ALL")
    os.environ["LC_ALL"] = "C"
    try:
        status = common.command(
            argv, output, log, args.cleanup_grace_seconds, args.cleanup_grace_seconds
        )
    finally:
        if previous is None:
            del os.environ["LC_ALL"]
        else:
            os.environ["LC_ALL"] = previous
    require(status == 0, f"{name} failed ({status})")
    return log.read_text().strip()


def domains(output, uri, args, phase):
    index = URIS.index(uri)
    return operation(
        ["virsh", "-c", uri, "list", "--all", "--name"],
        output,
        f"{phase}-domains-{index}",
        args,
    ).splitlines()


def check_network(output, config, args):
    xml = operation(
        ["virsh", "-c", URIS[0], "net-dumpxml", "default"],
        output,
        "network",
        args,
    )
    require(
        config["hostEndpointIP"]
        in [node.get("address") for node in ET.fromstring(xml).findall("./ip")],
        "Host endpoint must match the existing default libvirt network",
    )
    for port in (18080, 18081, 18082):
        with socket.socket() as probe:
            probe.bind((config["hostEndpointIP"], port))


def cleanup(output, owner, config, args):
    errors = []
    for uri in URIS:
        try:
            if owner not in domains(output, uri, args, "cleanup"):
                continue
            xml = operation(
                ["virsh", "-c", uri, "dumpxml", owner],
                output,
                f"cleanup-xml-{URIS.index(uri)}",
                args,
            )
            sources = [
                source.get("file")
                for source in ET.fromstring(xml).findall("./devices/disk/source")
            ]
            require(
                str(output / "artifacts/booted.qcow2") in sources,
                "Refusing cleanup of VM without suite-owned boot disk",
            )
            state = operation(
                ["virsh", "-c", uri, "domstate", owner],
                output,
                "cleanup-state",
                args,
            )
            if state != "shut off":
                operation(
                    ["virsh", "-c", uri, "destroy", owner],
                    output,
                    "cleanup-destroy",
                    args,
                )
            operation(
                ["virsh", "-c", uri, "undefine", owner, "--nvram"],
                output,
                "cleanup-undefine",
                args,
            )
            require(
                owner not in domains(output, uri, args, "verify"),
                "Owned VM remains after cleanup",
            )
        except (
            OSError,
            ValueError,
            ET.ParseError,
            subprocess.SubprocessError,
        ) as error:
            errors.append(str(error))
    try:
        unresolved = []
        starts = Path(config["starts"])
        require(
            starts.is_dir(),
            "Postgres creation journal unavailable; outer-host disposal required",
        )
        for intent in sorted(starts.glob("*.intent")):
            require(
                re.fullmatch(re.escape(owner) + r"-[0-9a-f]{32}", intent.stem)
                and intent.read_text(encoding="utf-8").strip() == intent.stem,
                "Invalid Postgres creation intent; outer-host disposal required",
            )
            receipt = intent.with_suffix(".created")
            if not receipt.exists():
                unresolved.append(intent.stem)
            else:
                require(
                    re.fullmatch(
                        r"[0-9a-f]{64}", receipt.read_text(encoding="utf-8").strip()
                    ),
                    "Invalid Postgres creation receipt; outer-host disposal required",
                )
        errors.extend(
            f"Unresolved Postgres creation {name}; outer-host disposal required"
            for name in unresolved
        )
        deadline = time.monotonic() + args.cleanup_grace_seconds

        def remaining_seconds():
            remaining = deadline - time.monotonic()
            require(remaining > 0, "Postgres cleanup reconciliation bound expired")
            return min(30, remaining)

        scan = ["ps", "-aq", "--no-trunc", "--filter", f"label={LABEL}={owner}"]
        while True:
            containers = docker_call(
                config["docker"], scan, timeout=remaining_seconds()
            ).splitlines()
            for container in containers:
                owned_container(config, container, timeout=remaining_seconds())
                docker_call(
                    config["docker"],
                    ["rm", "-f", container],
                    timeout=remaining_seconds(),
                )
            remaining = docker_call(config["docker"], scan, timeout=remaining_seconds())
            if not unresolved:
                require(not remaining, "Owned Postgres containers remain after cleanup")
                break
            # Empty scans never settle an interrupted request, even at the bound.
            delay = min(0.5, deadline - time.monotonic())
            if delay <= 0:
                break
            time.sleep(delay)
            if time.monotonic() >= deadline:
                break
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        # Do not stringify subprocess errors, which may contain argv/environment.
        errors.append(
            str(error)
            if isinstance(error, ValueError)
            else "Docker cleanup unavailable"
        )
    return errors


def execute(args):
    args.bundle = args.bundle.resolve()
    args.manifest = args.manifest.resolve()
    output = args.output.resolve()
    require(
        not output.is_relative_to(args.bundle)
        and not args.bundle.is_relative_to(output),
        "Output and bundle must not overlap",
    )
    output.mkdir(parents=True, exist_ok=False)
    owner = "trident-acl-" + uuid.uuid4().hex
    result = {
        "suite": "storm-aclagent-vm",
        "candidateRevision": args.expected_revision,
        "manifestSha256": args.manifest_sha256,
        "owner": owner,
        "selected": True,
        "invoked": False,
        "outcome": "unavailable",
        "cases": [{"name": name, "outcome": "not_run"} for name in CASES],
        "cleanupErrors": [],
    }
    status = 1
    try:
        manifest = verify(args)
        result["provenance"] = manifest
        config = manifest["aclAgent"]
        with common.host_lock():
            common.check_host()
            require(os.geteuid() == 0, "ACL-agent requires the existing root VM path")
            docker = shutil.which("docker")
            require(docker, "Docker prerequisite unavailable")
            for tool in ("virt-install", "ssh", "qemu-img"):
                require(shutil.which(tool), f"Missing prerequisite tool: {tool}")
            try:
                image_platform = docker_call(
                    docker,
                    [
                        "image",
                        "inspect",
                        "--format",
                        "{{.Os}}/{{.Architecture}}",
                        config["postgresImage"],
                    ],
                )
            except (OSError, ValueError, subprocess.SubprocessError):
                raise ValueError(
                    "Pinned Postgres unavailable in local Docker daemon; no pull/login attempted"
                ) from None
            require(
                image_platform == "linux/amd64",
                "Pinned Postgres must be linux/amd64, not an ARM64 lane",
            )
            check_network(output, config, args)
            for uri in URIS:
                require(
                    owner not in domains(output, uri, args, "preflight"),
                    "Suite VM name already exists",
                )
            service = {
                "docker": docker,
                "owner": owner,
                "image": config["postgresImage"],
            }
            artifacts = output / "artifacts"
            artifacts.mkdir()
            for path in (BASE, UPDATE):
                os.link(args.bundle / path, artifacts / Path(path).name)
            junit = output / "storm-aclagent.junit.xml"
            argv = [
                str(args.bundle / "bin/storm-trident"),
                "run",
                "aclagent",
                "-o",
                str(output),
                "-j",
                str(junit),
                "--",
                "--force-cleanup",
                "--output-path",
                str(output),
                "--artifacts-dir",
                str(artifacts),
                "--ssh-private-key-path",
                str(args.ssh_private_key_path.resolve()),
                "--platform",
                "qemu",
                "--name",
                owner,
                "--serial-log",
                str(output / "vm-serial.log"),
                "--postgres-image",
                config["postgresImage"],
                "--target-version",
                config["targetVersion"],
                "--host-endpoint-ip",
                config["hostEndpointIP"],
                "--api-server-port",
                "18080",
                "--nebraska-port",
                "18081",
                "--image-server-port",
                "18082",
                "--node-name",
                "trident-acl-agent-testimg",
                "--expected-initial-volume",
                "volume-a",
                "--user",
                "testuser",
                "--test-case-to-run",
                "all",
                "--image-pattern",
                r"^trident-vm-acl-agent-testimage\.qcow2$",
                "--cosi-pattern",
                r"^trident-vm-acl-agent-update-testimage\.cosi$",
            ]
            result["command"] = argv
            try:
                with service_environment(output, service):
                    result["invoked"] = True
                    result["outcome"] = "failed"
                    result["cases"] = [
                        {"name": name, "outcome": "unavailable"} for name in CASES
                    ]
                    status = common.command(
                        argv,
                        output,
                        output / "console.log",
                        args.timeout_seconds,
                        args.cleanup_grace_seconds,
                    )
                    result["processExitCode"] = status
                    cases = common.case_results(junit)
                    names = [case["name"] for case in cases]
                    require(
                        len(set(names)) == len(names) and set(names) <= set(CASES),
                        "Unexpected or duplicate ACL-agent cases",
                    )
                    reported = {case["name"]: case for case in cases}
                    result["cases"] = [
                        reported.get(name, {"name": name, "outcome": "unavailable"})
                        for name in CASES
                    ]
                    result["firstFailedCase"] = next(
                        (case["name"] for case in cases if case["outcome"] == "failed"),
                        None,
                    )
                    require(
                        names == list(CASES),
                        "Storm report does not match the existing ACL-agent cases",
                    )
                    if status == 0:
                        require(
                            all(case["outcome"] == "passed" for case in cases),
                            "ACL-agent success requires all existing cases to pass",
                        )
                        result["outcome"] = "passed"
            finally:
                with common.cleanup_signals():
                    result["cleanupErrors"] = cleanup(output, owner, service, args)
                if result["cleanupErrors"]:
                    status = status or 1
                    result["outcome"] = "failed"
    except KeyboardInterrupt:
        status = 130
        result["error"] = "Interrupted"
        result["outcome"] = "failed" if result["invoked"] else "unavailable"
    except (OSError, ValueError, KeyError, TypeError, ET.ParseError) as error:
        status = status or 1
        result["error"] = str(error)
    except subprocess.SubprocessError:
        status = status or 1
        result["error"] = "Bounded prerequisite/service command unavailable"
    finally:
        result["exitCode"] = status
        result["counts"] = {
            outcome: sum(case["outcome"] == outcome for case in result["cases"])
            for outcome in ("passed", "failed", "skipped", "unavailable", "not_run")
        }
        result["counts"]["selected"] = len(CASES)
        result["counts"]["executed"] = (
            result["counts"]["passed"] + result["counts"]["failed"]
        )
        (output / "results.json").write_text(json.dumps(result, indent=2) + "\n")
    return status


def main():
    if sys.argv[1:2] == ["docker-proxy"]:
        try:
            require(len(sys.argv) >= 4, "Missing Docker proxy configuration/operation")
            config = json.loads(Path(sys.argv[2]).read_text())
            print(docker_proxy(config, sys.argv[3:]))
            return 0
        except (OSError, ValueError, KeyError, subprocess.SubprocessError):
            print("ACL-agent owned Postgres operation unavailable", file=sys.stderr)
            return 1
    parser = argparse.ArgumentParser(description="Explicit existing ACL-agent VM suite")
    for name in ("bundle", "manifest", "output", "ssh-private-key-path"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--manifest-sha256", required=True)
    parser.add_argument("--expected-revision", required=True)
    for name in ("timeout-seconds", "cleanup-grace-seconds"):
        parser.add_argument("--" + name, type=common.positive_seconds, required=True)
    args = parser.parse_args()
    signal.signal(signal.SIGTERM, common.interrupt)
    try:
        return execute(args)
    except (OSError, ValueError) as error:
        print(error, file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
