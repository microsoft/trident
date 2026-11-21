#!/usr/bin/env python3
"""One-off real-runner validation. Never execute on a developer host."""

import hashlib
import json
import os
import re
import shlex
import subprocess
import sys
import tempfile
import time
import xml.etree.ElementTree as ET
from pathlib import Path

CANDIDATE = "3029aad73e28f71b1c47ca8eecdce9e6ec445c9f"
LOCK = Path("/var/lib/trident-cloud-agent/functional.lock")
PLANNED = (
    "candidate runner-baseline source-setup functional-setup-1 functional-setup-2 "
    "source-regressions fixture-regressions privileged-cross-uid format go "
    "host-lock-before smoke occupied ambiguous controlled-failure "
    "graceful-interruption host-lock-reacquire full"
).split()
PLUGIN = """
import os
import signal
import pytest
from pathlib import Path

def pytest_runtest_call(item):
    if item.name != "test_deployment":
        return
    item.funcargs["vm"].execute("true")
    Path(os.environ["HARDENING_MARKER"]).write_text(os.environ["HARDENING_INJECT"])
    print("HARDENING_REAL_VM_REACHED", flush=True)
    if os.environ["HARDENING_INJECT"] == "failure":
        pytest.fail("HARDENING_CONTROLLED_FAILURE")
    os.kill(os.getpid(), signal.SIGINT)
"""


def redact(text):
    text = re.sub(r"https?://[^\s<>\"']+", "[REDACTED_URL]", text)
    text = re.sub(
        r"(?im)^.*(?:authorization:|access[_-]?token|refresh[_-]?token|"
        r"client[_-]?secret|sharedaccesssignature|[?&]sig=).*$",
        "[REDACTED_CREDENTIAL_LINE]",
        text,
    )
    return re.sub(r"\beyJ[\w-]+\.[\w-]+\.[\w-]+\b", "[REDACTED_JWT]", text)


def sanitized(value):
    if isinstance(value, dict):
        return {key: sanitized(item) for key, item in value.items()}
    if isinstance(value, (list, tuple)):
        return [sanitized(item) for item in value]
    return redact(value) if isinstance(value, str) else value


def redact_junit(text):
    root = ET.fromstring(text)
    for element in root.iter():
        element.attrib.update(sanitized(element.attrib))
        if element.text:
            element.text = redact(element.text)
        if element.tail:
            element.tail = redact(element.tail)
    return ET.tostring(root, encoding="unicode")


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def snapshot(paths):
    result = {}
    for path in paths:
        path = Path(path)
        if path.is_symlink():
            result[str(path)] = {
                "link": os.readlink(path),
                "sha256": digest(path) if path.is_file() else None,
            }
        elif path.is_file():
            result[str(path)] = {"sha256": digest(path), "mode": path.stat().st_mode}
        elif path.is_dir():
            result[str(path)] = snapshot(sorted(path.iterdir()))
        else:
            result[str(path)] = None
    return result


def lock_identity():
    # usermod does not refresh this process's supplementary libvirt group.
    script = """
import json
import os
import sys

try:
    metadata = os.stat(sys.argv[1])
except FileNotFoundError:
    print("null")
else:
    print(json.dumps([metadata.st_dev, metadata.st_ino]))
"""
    return json.loads(
        subprocess.check_output(
            ["sudo", "python3", "-c", script, str(LOCK)],
            text=True,
            stderr=subprocess.PIPE,
        )
    )


def junit_summary(text):
    root = ET.fromstring(text)
    cases = list(root.iter("testcase"))
    passed = [
        case
        for case in cases
        if not any(
            case.find(tag) is not None for tag in ("failure", "error", "skipped")
        )
    ]
    return {
        "tests": len(cases),
        "passed": len(passed),
        **{
            tag: sum(case.find(tag) is not None for case in cases)
            for tag in ("failure", "error", "skipped")
        },
        "vm_created": any(case.get("name") == "test_create_vm" for case in passed),
        "ssh_deployment": any(case.get("name") == "test_deployment" for case in passed),
        "rust_functional_passed": sum(
            case.get("classname", "").startswith(
                ("ft.json.", "tests.functional_tests.ft.json.")
            )
            for case in passed
        ),
    }


class Validation:
    def __init__(self, source, reports):
        self.source = Path(source).resolve()
        self.reports = Path(reports).resolve()
        self.reports.mkdir(mode=0o700, parents=True, exist_ok=True)
        self.scratch = Path(tempfile.mkdtemp(prefix="trident-hardening-"))
        self.results = {}
        self.env = dict(os.environ)
        self.env["PATH"] = (
            f"{Path.home()}/.local/bin:{Path.home()}/.cargo/bin:" + self.env["PATH"]
        )
        self.env["LIBVIRT_DEFAULT_URI"] = "qemu:///system"
        self.env["LC_ALL"] = "C"
        self.env["PYTEST_ADDOPTS"] = ""
        self.namespace = (
            f"hardening-{os.environ['GITHUB_RUN_ID']}-"
            f"{os.environ['GITHUB_RUN_ATTEMPT']}-{os.getpid()}"
        )

    def save(self, name, value):
        (self.reports / name).write_text(
            json.dumps(sanitized(value), indent=2) + "\n", encoding="utf-8"
        )

    def run(self, name, args, expected=0, env=None):
        started = time.monotonic()
        output = []
        with (self.reports / f"{name}.log").open("w", encoding="utf-8") as log:
            log.write(redact(shlex.join(args)) + "\n")
            with subprocess.Popen(
                args,
                cwd=self.source,
                env=env or self.env,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
                errors="replace",
            ) as process:
                for line in process.stdout:
                    safe = redact(line)
                    log.write(safe)
                    log.flush()
                    output.append(safe)
                code = process.wait()
        text = "".join(output)
        self.results[name] = {
            "exit": code,
            "expected_exit": expected,
            "seconds": round(time.monotonic() - started, 1),
            "unittest_counts": re.findall(
                r"Ran \d+ tests?[^\n]*|OK(?: \([^\n]*\))?|FAILED \([^\n]*\)", text
            ),
        }
        self.save("results.json", self.results)
        print(f"{name}: exit {code} (expected {expected})", flush=True)
        if code != expected:
            if not (self.reports / "first-failure.json").exists():
                self.save("first-failure.json", {"step": name, **self.results[name]})
            raise RuntimeError(f"{name}: exit {code}, see redacted {name}.log")
        return text

    def virsh(self, *args):
        return subprocess.check_output(
            ["sudo", "env", "LC_ALL=C", "virsh", "-c", "qemu:///system", *args],
            text=True,
            stderr=subprocess.PIPE,
        )

    def inventory(self):
        result = {}
        for kind, listing, dump in (
            ("domains", "list", "dumpxml"),
            ("networks", "net-list", "net-dumpxml"),
            ("pools", "pool-list", "pool-dumpxml"),
        ):
            result[kind] = {}
            for name in sorted(self.virsh(listing, "--all", "--name").split()):
                xml = self.virsh(dump, name)
                entry = {"xml_sha256": hashlib.sha256(xml.encode()).hexdigest()}
                if kind == "domains":
                    entry["state"] = self.virsh("domstate", name).strip()
                if kind == "pools":
                    info = self.virsh("pool-info", name)
                    entry["state"] = re.search(r"^State:\s+(.+)$", info, re.M)[1]
                    entry["target"] = ET.fromstring(xml).findtext("target/path")
                    if entry["state"] == "running":
                        entry["volumes"] = {
                            volume: hashlib.sha256(
                                self.virsh(
                                    "vol-dumpxml", volume, "--pool", name
                                ).encode()
                            ).hexdigest()
                            for volume in self.virsh(
                                "vol-list", name, "--name"
                            ).splitlines()
                            if volume.strip()
                        }
                    else:
                        entry["retained_inactive_pool_files"] = entry["target"]
                result[kind][name] = entry
        return result

    def assert_clean(self, namespace):
        inventory = self.inventory()
        remaining = [
            name
            for resources in inventory.values()
            for name in resources
            if name.startswith(namespace + "-")
        ]
        if remaining:
            raise RuntimeError(f"Run-owned resources remain: {remaining}")

    def functional(self, name, mode, expected=0, inject=None, namespace=None):
        namespace = namespace or f"{self.namespace}-{name}"
        env = dict(self.env, TRIDENT_FUNCTIONAL_NAMESPACE=namespace)
        if inject:
            marker = self.scratch / "injection-reached"
            if marker.exists():
                marker.unlink()
            env.update(
                PYTHONPATH=str(self.scratch),
                PYTEST_ADDOPTS="-p hardening_inject -x",
                HARDENING_INJECT=inject,
                HARDENING_MARKER=str(marker),
            )
        report = self.source / "target/trident_functional_tests.xml"
        before = report.stat().st_mtime_ns if report.exists() else None
        args = (
            ["bash", "scripts/cloud-agent/validate.sh", "functional"]
            if name == "smoke"
            else ["bash", "scripts/cloud-agent/run-functional.sh", mode]
        )
        try:
            text = self.run(name, args, expected=expected, env=env)
        finally:
            if report.exists() and report.stat().st_mtime_ns != before:
                safe = redact_junit(report.read_text())
                # XML parsing precedes publication, malformed redaction must fail.
                counts = junit_summary(safe)
                (self.reports / f"{name}.xml").write_text(safe)
                self.results.setdefault(name, {})["junit"] = counts
                self.save("results.json", self.results)
            self.assert_clean(namespace)
        counts = self.results.get(name, {}).get("junit")
        if not counts:
            raise RuntimeError(f"{name}: no fresh JUnit, cannot count execution")
        if inject:
            # The interrupt can prevent captured stdout publication. Successful
            # VM creation plus exit 2 is not sufficient, require plugin evidence.
            marker = self.scratch / "injection-reached"
            if (
                not marker.exists()
                or marker.read_text() != inject
                or not counts["vm_created"]
            ):
                raise RuntimeError(f"{name}: injection did not reach a real VM")
            if inject == "failure" and "HARDENING_CONTROLLED_FAILURE" not in text:
                raise RuntimeError("Controlled pytest failure was not observed")
        elif not (
            counts["vm_created"]
            and counts["ssh_deployment"]
            and counts["rust_functional_passed"] > 0
            and counts["failure"] == counts["error"] == 0
        ):
            raise RuntimeError(f"{name}: real VM/SSH/Rust test gate failed: {counts}")
        return text

    def provenance(self, text):
        match = re.search(r"--functional-base-image (\S+)", text)
        if not match:
            raise RuntimeError("No immutable functional image path recorded")
        image = Path(match[1])
        raw = json.loads((image.parent / "provenance.json").read_text())
        summary = {
            "build_id": raw["build"]["id"],
            "source_commit": raw["build"]["sourceVersion"],
            "image_sha256": raw["image"]["sha256"],
            "image_size": raw["image"]["size"],
            "cache_hit": raw["cacheHit"],
        }
        if not re.fullmatch(r"[a-fA-F0-9]{64}", summary["image_sha256"]):
            raise RuntimeError("Invalid provenance image digest")
        if not re.fullmatch(r"[a-fA-F0-9]{40}", summary["source_commit"]):
            raise RuntimeError("Invalid provenance source commit")
        self.save("provenance.json", summary)
        self.env["TRIDENT_FUNCTIONAL_BUILD_ID"] = str(summary["build_id"])
        return image, summary

    def host_lock(self, name):
        shared = [
            self.source / "tests/functional_tests/ft.json",
            self.source / "tools/vm-netlaunch.yaml",
        ]
        before = snapshot(shared)
        command = (
            f"exec 9<{LOCK}; flock -n 9 || exit; printf 'locked\\n'; read -r release"
        )
        holder = subprocess.Popen(
            ["sg", "libvirt", "-c", command],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        try:
            if holder.stdout.readline() != "locked\n":
                raise RuntimeError("Could not hold real host lock")
            text = self.run(
                name,
                ["bash", "scripts/cloud-agent/run-functional.sh", "smoke"],
                expected=1,
            )
            if "Host is busy with another functional run" not in text:
                raise RuntimeError("Competing runner did not reject host lock")
            if "download_functional_artifact.py" in text or before != snapshot(shared):
                raise RuntimeError("Competing runner reached shared preparation")
        finally:
            holder.communicate("release\n", timeout=10)
        if holder.returncode != 0:
            raise RuntimeError("Host lock holder failed")

    def occupied(self):
        namespace = f"{self.namespace}-sentinel"
        network = namespace + "-network"
        xml = self.scratch / "sentinel-network.xml"
        xml.write_text(f"<network><name>{network}</name></network>")
        self.assert_clean(namespace)
        self.virsh("net-define", str(xml))
        try:
            before = self.inventory()
            for suffix in ("", "-nested"):
                env = dict(self.env, TRIDENT_FUNCTIONAL_NAMESPACE=namespace + suffix)
                name = "occupied" if not suffix else "ambiguous"
                text = self.run(
                    name,
                    ["bash", "scripts/cloud-agent/run-functional.sh", "smoke"],
                    expected=1,
                    env=env,
                )
                if "conflicts with existing resource" not in text:
                    raise RuntimeError(f"{name}: namespace preflight did not reject")
                if self.inventory() != before:
                    raise RuntimeError(f"{name}: libvirt sentinel/inventory changed")
        finally:
            # Only the inactive network defined above is ours to undefine.
            self.virsh("net-undefine", network)

    def execute(self):
        if os.environ.get("GITHUB_ACTIONS") != "true":
            raise RuntimeError("This harness may only execute on GitHub Actions")
        self.run("candidate", ["git", "rev-parse", "HEAD"])
        actual = subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=self.source, text=True
        ).strip()
        if actual != CANDIDATE:
            raise RuntimeError("Candidate SHA does not match the validation contract")
        self.run(
            "runner-baseline",
            [
                "bash",
                "-ec",
                "cat /etc/os-release; uname -a; id; "
                'test "$( . /etc/os-release; echo "$ID:$VERSION_ID" )" = ubuntu:22.04; '
                "test -c /dev/kvm; ls -l /dev/kvm; "
                "mountpoint -q /mnt/storage; findmnt --mountpoint /mnt/storage; "
                "go version; python3 --version; "
                "az version --query '\"azure-cli\"' -o tsv",
            ],
        )
        self.save(
            "identity.json",
            {
                "candidate": CANDIDATE,
                "harness_sha": os.environ["GITHUB_SHA"],
                "runner_label": "trident-github-runner-ubuntu2204-amd64",
                "runner_os": os.environ.get("RUNNER_OS"),
                "startup_setup": "Not assumed. Explicit normal setup below is authoritative.",
            },
        )
        config = Path.home() / ".config/libvirt/libvirt.conf"
        protected = [
            self.source / "tools/vm-netlaunch.yaml",
            self.source / ".pytest_cache",
            self.source / "tests/functional_tests/.pytest_cache",
            config,
        ]
        # Track existing files, not new keys which setup is allowed to create.
        protected += [
            path
            for path in (Path.home() / ".ssh").rglob("*")
            if path.is_file() or path.is_symlink()
        ]
        original = config.read_bytes() if config.exists() else None
        baseline = snapshot(protected)
        self.save("files-original.json", baseline)
        self.save("ssh-original.json", snapshot([Path.home() / ".ssh"]))
        config.parent.mkdir(parents=True, exist_ok=True)
        sentinel = b"\n# trident-hardening-validation preservation sentinel\n"
        config.write_bytes((original or b"") + sentinel)
        expected = snapshot(protected)
        inventory = None
        lock_inode = None
        try:
            lock_inode = lock_identity()
            self.save("lock-inode-original.json", lock_inode)
            self.run("source-original-status", ["git", "status", "--short"])
            self.run(
                "source-original-diff", ["git", "diff", "--exit-code", "HEAD", "--"]
            )
            # Capture pre-setup libvirt state when already available. A new
            # runner may not have libvirt installed/running until optional setup.
            try:
                inventory = self.inventory()
                self.save("inventory-pre-setup.json", inventory)
            except (FileNotFoundError, subprocess.CalledProcessError) as error:
                self.save(
                    "inventory-pre-setup.json",
                    {
                        "unavailable": type(error).__name__,
                        "limitation": "Pre-setup libvirt preservation cannot be proved.",
                    },
                )
            self.run("source-setup", ["bash", "scripts/cloud-agent/setup.sh"])
            self.env["PATH"] = (
                subprocess.check_output(["go", "env", "GOPATH"], text=True).strip()
                + "/bin:"
                + self.env["PATH"]
            )
            for iteration in (1, 2):
                self.run(
                    f"functional-setup-{iteration}",
                    ["bash", "scripts/cloud-agent/setup-functional.sh"],
                )
                inode = lock_identity()
                if inode is None:
                    raise RuntimeError("Functional setup did not provision the lock")
                if lock_inode is not None and inode != lock_inode:
                    raise RuntimeError(
                        "Functional setup replaced persistent lock inode"
                    )
                lock_inode = inode
                current = snapshot(protected)
                self.save(f"files-setup-{iteration}.json", current)
                if current != expected:
                    raise RuntimeError(f"Setup {iteration} changed protected files")
                if iteration == 1:
                    protected += [
                        path
                        for path in (Path.home() / ".ssh").rglob("*")
                        if path not in protected
                        and (path.is_file() or path.is_symlink())
                    ]
                    expected = snapshot(protected)
            self.save("lock-inode.json", lock_inode)
            self.run("xorrisofs", ["xorrisofs", "-version"])
            self.run("libvirt-version", ["virsh", "--version"])
            self.run("qemu-version", ["qemu-system-x86_64", "--version"])
            setup_inventory = self.inventory()
            self.save("inventory-before.json", setup_inventory)
            if inventory is not None and inventory != setup_inventory:
                raise RuntimeError(
                    "Functional setup changed existing libvirt inventory"
                )
            inventory = setup_inventory
            checks = {
                "source-regressions": "bash scripts/cloud-agent/validate.sh cloud-agent",
                "fixture-regressions": "python3 -m unittest discover -s tests/functional_tests -t tests -p test_fixtures.py",
                "privileged-cross-uid": "sudo python3 -m unittest discover -s scripts/cloud-agent/tests -p test_run_functional.py -v",
                "format": "bash scripts/cloud-agent/validate.sh format",
                "go": "bash -ec 'cd tools; go generate pkg/rcp/tlscerts/certs.go; go generate pkg/tridentgrpc/grpc.go; go test -mod=readonly ./...; go test -mod=readonly -race ./pkg/virtdeploy/...'",
            }
            for name, command in checks.items():
                text = self.run(name, shlex.split(command))
                if name == "privileged-cross-uid" and (
                    "skipped" in text.lower()
                    or not re.search(
                        r"test_provisioned_lock_reopens_across_uids_with_protected_regular .* ok",
                        text,
                    )
                ):
                    raise RuntimeError(
                        "Privileged cross-UID test did not actually pass"
                    )
            self.host_lock("host-lock-before")
            smoke = self.functional("smoke", "smoke")
            image, provenance = self.provenance(smoke)
            self.save("smoke-provenance.json", provenance)
            identity = (
                image.stat().st_ino,
                image.stat().st_mtime_ns,
                image.stat().st_size,
            )
            # Exercise bounded failure paths before the potentially long suite.
            self.occupied()
            (self.scratch / "hardening_inject.py").write_text(PLUGIN)
            self.functional("controlled-failure", "smoke", expected=1, inject="failure")
            self.functional(
                "graceful-interruption", "smoke", expected=2, inject="interrupt"
            )
            self.host_lock("host-lock-reacquire")
            full = self.functional("full", "full")
            full_image, cached = self.provenance(full)
            unchanged = identity == (
                image.stat().st_ino,
                image.stat().st_mtime_ns,
                image.stat().st_size,
            )
            if full_image != image or not cached["cache_hit"] or not unchanged:
                raise RuntimeError("Full repeat did not reuse immutable cached image")
            self.save("cache-reuse.json", {**provenance, "cache_hit": True})
        finally:
            original_failure = sys.exc_info()[0] is not None
            after = snapshot(protected)
            self.save("files-before.json", expected)
            self.save("files-after.json", after)
            files_ok = expected == after
            # Restore only our own exact sentinel edit, never overwrite a surprise.
            if after[str(config)] == expected[str(config)] and config.is_file():
                if original is None:
                    config.unlink()
                else:
                    config.write_bytes(original)
            inventories_ok = None
            if inventory is not None:
                final = self.inventory()
                self.save("inventory-after.json", final)
                inventories_ok = final == inventory
            inode_ok = None
            if lock_inode is not None:
                inode_ok = lock_inode == lock_identity()
            self.save(
                "preservation.json",
                {
                    "files_preserved": files_ok,
                    "inventory_preserved": inventories_ok,
                    "lock_inode_preserved": inode_ok,
                    "private_diagnostic_scratch": str(self.scratch),
                    "unexecuted_gates": [
                        "SIGKILL cleanup (exit traps cannot execute)",
                        "Live lock contention during cleanup (only source regression scheduled)",
                        "Occupied domain/pool/volume sentinels (only network sentinel scheduled live)",
                    ],
                },
            )
            self.run("source-final-status", ["git", "status", "--short"])
            if not (files_ok and inventories_ok and inode_ok) and not original_failure:
                raise RuntimeError("Before/after preservation gate failed")


def main():
    validation = Validation(*sys.argv[1:])
    status = "FAILED"
    try:
        validation.execute()
        status = "PASSED executed gates, see explicitly unexecuted gates"
        return 0
    except (Exception, KeyboardInterrupt) as error:
        validation.save(
            "failure.json",
            {
                "type": type(error).__name__,
                "message": str(error),
                "stderr": getattr(error, "stderr", None),
            },
        )
        print(redact(f"Validation failed: {error}"), file=sys.stderr)
        return 1
    finally:
        validation.save("results.json", validation.results)
        rows = [
            f"## Hardening validation: {status}",
            f"Candidate: `{CANDIDATE}`",
            "Skipped tests are recorded separately and are not passed tests.",
            "See preservation.json for before/after gates and unexecuted scenarios.",
        ]
        for name, result in validation.results.items():
            rows.append(f"- {name}: `{json.dumps(result, sort_keys=True)}`")
        missing = [name for name in PLANNED if name not in validation.results]
        rows.append(f"Not run: {', '.join(missing) or 'none of the scheduled steps'}")
        validation.save("not-run.json", missing)
        (validation.reports / "summary.md").write_text(redact("\n".join(rows)) + "\n")


if __name__ == "__main__":
    sys.exit(main())
