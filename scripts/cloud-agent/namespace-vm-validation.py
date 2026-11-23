#!/usr/bin/env python3
"""One-off prerequisite check, only for an exclusive disposable 1ES Linux host."""

import hashlib
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys

CANDIDATE = "ed0688f628ef5be84a6c06e555ed317ffd4ee49c"
VM_CASE = "TestFailureArtifactsSurviveVMCleanup"
VM_PACKAGE = "tridenttools/storm/e2e/scenario"
REGRESSIONS = {
    "TestMatchesDomainName",
    "TestPoolTargetDisambiguatesNamespace",
    "TestCleanupNamespaceSelectsCanonicalResources",
    "TestCleanupNamespaceRejectsPoolConflictsBeforeDeletion",
    "TestCleanupNamespacePropagatesErrors",
    "TestCleanupNamespaceWithNoSelectedResources",
}
VM_PROOF = (
    "intentional product failure preserved; disk and SSH key survived VM cleanup; "
    "disk checksum matches"
)


def case_results(text, package, expected, vm=False):
    """Count outer Go events only, not the intentionally failing Storm subprocess."""
    events = [json.loads(line) for line in text.splitlines() if line.strip()]
    runs, outcomes, output = [], {}, ""
    package_passes = 0
    for event in events:
        if event.get("Package") != package:
            raise ValueError("unexpected Go package")
        name, action = event.get("Test"), event["Action"]
        if name and action == "run":
            runs.append(name)
        if name and action in ("pass", "fail", "skip"):
            if name in outcomes:
                raise ValueError("duplicate case outcome")
            outcomes[name] = action
        if not name and action == "pass":
            package_passes += 1
        if action in ("skip", "fail"):
            raise ValueError(f"unintended {action}: {name or package}")
        if name == VM_CASE and action == "output":
            output += event.get("Output", "")
    if (
        len(runs) != len(set(runs))
        or set(runs) != set(outcomes)
        or {name for name in runs if "/" not in name} != set(expected)
        or package_passes != 1
        or any(action != "pass" for action in outcomes.values())
        or (vm and VM_PROOF not in output)
    ):
        raise ValueError(
            "missing execution, parent success marker, or case/package pass"
        )
    return {"passed": len(outcomes), "skipped": 0, "cases": outcomes}


class Validation:
    def __init__(self, source, reports):
        self.source = Path(source).resolve()
        self.reports = Path(reports).resolve()
        self.reports.mkdir(parents=True, exist_ok=True)
        self.env = {
            key: value
            for key, value in os.environ.items()
            if not key.startswith("TRIDENT_TEST_FAILURE_VM")
        }
        self.env.update(LC_ALL="C", TF_BUILD="False", GOFLAGS="", GOTOOLCHAIN="local")

    def save(self, name, value):
        (self.reports / name).write_text(
            json.dumps(value, indent=2) + "\n", encoding="utf-8"
        )

    def run(self, name, args, cwd=None):
        # Stream to disk so a timeout still leaves partial evidence.
        suffix = ".jsonl" if name in ("virtdeploy", "failure-vm") else ".log"
        path = self.reports / (name + suffix)
        metadata = {"command": list(map(str, args)), "exit": None}
        self.save(f"{name}-exit.json", metadata)
        # Go's first dependency downloads go to stderr, not the JSON event stream.
        with path.open("w", encoding="utf-8") as log, (
            self.reports / f"{name}.stderr.log"
        ).open("w", encoding="utf-8") as stderr:
            result = subprocess.run(
                args,
                cwd=cwd or self.source,
                env=self.env,
                stdout=log,
                stderr=stderr,
            )
        metadata["exit"] = result.returncode
        self.save(f"{name}-exit.json", metadata)
        text = path.read_text(encoding="utf-8")
        if result.returncode:
            raise RuntimeError(f"{name}: exit {result.returncode}, see {path.name}")
        return text

    def inventory(self, phase):
        inventory = {}
        for kind, command in (
            ("domains", "list"),
            ("pools", "pool-list"),
            ("networks", "net-list"),
        ):
            inventory[kind] = sorted(
                self.run(
                    f"{phase}-{kind}",
                    shlex.split(
                        f"sudo -n virsh -c qemu:///system {command} --all --name"
                    ),
                ).split()
            )
        inventory["fixture_directories"] = sorted(
            self.run(
                f"{phase}-fixture-directories",
                shlex.split(
                    "sudo -n find /var/lib/libvirt -mindepth 1 -maxdepth 1 "
                    "-type d -name 'storm-capture-test-*' -print"
                ),
            ).splitlines()
        )
        self.save(f"inventory-{phase}.json", inventory)
        return inventory

    def clean_source(self, phase):
        head = self.run(f"{phase}-head", ["git", "rev-parse", "HEAD"]).strip()
        status = self.run(
            f"{phase}-status",
            ["git", "status", "--porcelain=v1", "--untracked-files=all"],
        )
        if head != CANDIDATE or status.strip():
            raise RuntimeError(f"{phase}: candidate pin or source cleanliness mismatch")

    def validate(self):
        self.save(
            "provenance.json",
            {
                "candidate": CANDIDATE,
                "harness": os.environ["GITHUB_SHA"],
                "runner": os.environ.get("RUNNER_NAME"),
                "runner_label": "trident-github-runner-ubuntu2204-amd64",
                "scope": "prerequisite only, no guest image integration or full E2E",
                "setup_source": CANDIDATE,
            },
        )
        self.clean_source("before")
        # Only the paused, BIOS, 4 MiB qcow2 fixture runs. No OVMF, swtpm,
        # xorriso, Rust, Azure login, or older functional orchestrator is needed.
        self.run(
            "apt-update",
            shlex.split("sudo -n apt-get -o DPkg::Lock::Timeout=300 update"),
        )
        self.run(
            "apt-install",
            shlex.split(
                "sudo -n env DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=a "
                "apt-get -o DPkg::Lock::Timeout=300 install -y --no-install-recommends "
                "build-essential curl unzip libvirt-clients libvirt-daemon-system "
                "qemu-kvm qemu-utils"
            ),
        )
        self.run(
            "libvirt-start", shlex.split("sudo -n systemctl start libvirtd.service")
        )
        self.run(
            "root-kvm",
            [
                "sudo",
                "-n",
                "python3",
                "-c",
                "import os,stat; assert os.geteuid()==0; "
                "assert stat.S_ISCHR(os.stat('/dev/kvm').st_mode); "
                "fd=os.open('/dev/kvm',os.O_RDWR); os.close(fd); "
                "print('euid=0; KVM opened read/write')",
            ],
        )
        virsh = shlex.split("sudo -n virsh -c qemu:///system")
        self.run("libvirt-version", virsh + ["version"])
        self.run("kvm-capabilities", virsh + ["domcapabilities", "--virttype", "kvm"])
        self.run("qemu-version", ["qemu-system-x86_64", "--version"])
        self.run("qemu-img-version", ["qemu-img", "--version"])
        before = self.inventory("before")
        if before["domains"] or before["fixture_directories"]:
            raise RuntimeError(
                "expected disposable host without domains or fixture remnants"
            )
        self.run("protoc-setup", ["make", "install-protoc"])
        self.run(
            "plugin-go",
            shlex.split(
                "go install google.golang.org/protobuf/cmd/protoc-gen-go@v1.36.11"
            ),
        )
        self.run(
            "plugin-grpc",
            shlex.split(
                "go install google.golang.org/grpc/cmd/protoc-gen-go-grpc@v1.6.2"
            ),
        )
        gopath = self.run("gopath", ["go", "env", "GOPATH"]).strip()
        self.env["PATH"] = gopath + "/bin:" + self.env["PATH"]
        self.run("go-version", ["go", "version"])
        self.run("protoc-version", ["protoc", "--version"])
        self.run(
            "generate-grpc",
            ["go", "generate", "pkg/tridentgrpc/grpc.go"],
            self.source / "tools",
        )
        text = self.run(
            "virtdeploy",
            shlex.split("go test -mod=readonly -json -count=1 -timeout=3m")
            + ["-run=^(" + "|".join(sorted(REGRESSIONS)) + ")$", "./pkg/virtdeploy"],
            self.source / "tools",
        )
        self.save(
            "virtdeploy-cases.json",
            case_results(text, "tridenttools/pkg/virtdeploy", REGRESSIONS),
        )
        binary = Path(os.environ["RUNNER_TEMP"]) / "namespace-failure-vm.test"
        self.run(
            "build-vm",
            shlex.split("go test -mod=readonly -c -o")
            + [str(binary), "./storm/e2e/scenario"],
            self.source / "tools",
        )
        self.save(
            "binary.json",
            {
                "sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
                "candidate": CANDIDATE,
            },
        )
        self.run("binary-build-info", ["go", "version", "-m", str(binary)])
        text = self.run(
            "failure-vm",
            shlex.split("sudo -n env TRIDENT_TEST_FAILURE_VM=1 TF_BUILD=False")
            + [
                shutil.which("go"),
                "tool",
                "test2json",
                "-t",
                "-p",
                VM_PACKAGE,
                str(binary),
                "-test.v=test2json",
                "-test.count=1",
                "-test.timeout=5m",
                "-test.run=^" + VM_CASE + "$",
            ],
            self.source / "tools",
        )
        self.save(
            "failure-vm-cases.json", case_results(text, VM_PACKAGE, {VM_CASE}, vm=True)
        )

    def audit(self):
        errors = []
        try:
            after = self.inventory("after")
            before = json.loads((self.reports / "inventory-before.json").read_text())
            if before != after:
                errors.append(
                    "TEARDOWN FAILURE: libvirt inventory or fixture directories changed"
                )
        except Exception as error:
            errors.append(f"TEARDOWN UNVERIFIED: {error}")
        try:
            self.clean_source("after")
            self.run(
                "after-ignored-generated-files",
                shlex.split("git ls-files --others --ignored --exclude-standard"),
            )
        except Exception as error:
            errors.append(str(error))
        # Cleanup belongs solely to the fixture's UUID namespace. Never sweep libvirt.
        self.save(
            "audit.json",
            {
                "passed": not errors,
                "errors": errors,
                "cleanup_owner": "candidate fixture t.Cleanup",
                "limitation": "No cleanup guarantee after SIGKILL or host loss",
            },
        )
        if errors:
            raise RuntimeError("; ".join(errors))


if __name__ == "__main__":
    mode, source, reports = sys.argv[1:]
    validation = Validation(source, reports)
    if mode == "run":
        validation.validate()
    elif mode == "audit":
        validation.audit()
    else:
        raise ValueError("expected run or audit")
