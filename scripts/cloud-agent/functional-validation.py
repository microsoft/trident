#!/usr/bin/env python3
"""Temporary functional-only proof on an exclusive disposable 1ES runner."""

import hashlib
import json
import os
import re
import shlex
import signal
import subprocess
import sys
import tempfile
import time
import xml.etree.ElementTree as ET
from collections import Counter
from pathlib import Path

SOURCE = "985a0a70b88ea5576805c6fccae6a6ff4d8800a4"
PREREQ = "ed0688f628ef5be84a6c06e555ed317ffd4ee49c"
LOCK = "/var/lib/trident-cloud-agent/functional.lock"
MAX_LOG = 24 * 1024 * 1024
GO_BASELINE_TEST = "TestUefiFallbackScriptMatchesLegacy"
GO_BASELINE_LIMIT = (
    f"Go test {GO_BASELINE_TEST} excluded/not validated; full Go suite not validated. "
    "Pre-existing embedded-script drift on clean 843f411 (parent verified); "
    "observed in run 45248620733, go.log lines 159-160."
)
PLANNED = (
    "source-identity prerequisite-identity source-before source-setup-1 "
    "functional-setup-1 source-setup-2 functional-setup-2 source-tests "
    "fixture-tests privileged-lock-tests format go-filtered smoke full "
    "controlled-failure graceful-interruption source-after"
).split()

# Observe the existing uploader, do not replace its implementation. The source
# runner supplies --force-upload and the pinned checkout produces these binaries.
PLUGIN = """
import hashlib
import json
import os
import signal
from pathlib import Path
import pytest

def pytest_sessionstart(session):
    binaries = []
    for line in Path(session.config.getoption("--build-output")).read_text().splitlines():
        report = json.loads(line)
        if ("lib" in report.get("target", {}).get("kind", [])
                and report.get("executable")):
            path = Path(report["executable"])
            digest = hashlib.sha256()
            with path.open("rb") as stream:
                for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                    digest.update(chunk)
            binaries.append({"local": str(path),
                             "remote": "tests/" + path.name.split("-", 2)[0],
                             "sha256": digest.hexdigest(), "size": path.stat().st_size})
    Path(os.environ["VALIDATION_BINARIES"]).write_text(json.dumps(binaries))

def pytest_runtest_call(item):
    inject = os.environ.get("VALIDATION_INJECT")
    if not inject or item.name != "test_deployment":
        return
    item.funcargs["vm"].execute("true").assert_exit_code()
    Path(os.environ["VALIDATION_MARKER"]).write_text(inject)
    if inject == "failure":
        pytest.fail("VALIDATION_CONTROLLED_FAILURE")
    os.kill(os.getpid(), signal.SIGINT)
"""

INVENTORY = """
import hashlib, json, libvirt
import xml.etree.ElementTree as ET
def definition(xml):
    root = ET.fromstring(xml)
    for tag in ("allocation", "physical", "capacity", "available", "target/timestamps"):
        for node in root.findall(tag):
            parent = root.find("target") if "/" in tag else root
            parent.remove(node)
    return hashlib.sha256(ET.tostring(root)).hexdigest()
connection = libvirt.openReadOnly("qemu:///system")
try:
    result = {}
    for kind, objects in (
        ("domains", connection.listAllDomains(0)),
        ("networks", connection.listAllNetworks(0)),
        ("pools", connection.listAllStoragePools(0)),
    ):
        result[kind] = {}
        for obj in objects:
            entry = {"active": bool(obj.isActive()), "xml_sha256": definition(obj.XMLDesc(0))}
            if kind == "pools" and obj.isActive():
                entry["volumes"] = {v.name(): definition(v.XMLDesc(0))
                                    for v in obj.listAllVolumes(0)}
            result[kind][obj.name()] = entry
    print(json.dumps(result, sort_keys=True))
finally:
    connection.close()
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


def digest(path):
    result = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            result.update(chunk)
    return result.hexdigest()


def sanitized(value):
    if isinstance(value, dict):
        return {key: sanitized(item) for key, item in value.items()}
    if isinstance(value, list):
        return [sanitized(item) for item in value]
    return redact(value) if isinstance(value, str) else value


def snapshot(paths):
    result = {}
    for path in paths:
        if path.is_dir() and not path.is_symlink():
            result[str(path)] = snapshot(sorted(path.iterdir()))
        elif path.exists() or path.is_symlink():
            result[str(path)] = {
                "sha256": digest(path) if path.is_file() else None,
                "mode": path.lstat().st_mode,
                "link": os.readlink(path) if path.is_symlink() else None,
            }
        else:
            result[str(path)] = None
    return result


def manifest_cases(data, mode, prefix=()):
    result = []
    for name, info in data.get("test_cases", {}).items():
        marks = info.get("markers", [])
        if mode == "smoke" and not (
            ("helpers" in marks and "positive" in marks) or "functional" not in marks
        ):
            continue
        if info.get("skip") or info.get("xfail"):
            raise RuntimeError(f"Unexpected manifest skip/xfail: {prefix + (name,)}")
        result.append((".".join(prefix), name))
    for name, child in data.get("submodules", {}).items():
        result.extend(manifest_cases(child, mode, prefix + (name,)))
    return result


def verify_junit(root, manifest, mode):
    cases = list(root.iter("testcase"))
    if not cases or any(
        case.find(tag) is not None
        for case in cases
        for tag in ("failure", "error", "skipped")
    ):
        raise RuntimeError("Missing tests or unexpected failed/errored/skipped JUnit")
    names = {case.get("name") for case in cases}
    if not {"test_create_vm", "test_deployment"} <= names:
        raise RuntimeError("Real VM creation and SSH deployment did not pass")
    expected = Counter(
        item
        for crate, data in manifest.items()
        for item in manifest_cases(data, mode, (crate,))
    )
    actual = Counter()
    for case in cases:
        classname = case.get("classname", "")
        for prefix in ("ft.json.", "tests.functional_tests.ft.json."):
            if classname.startswith(prefix):
                actual[(classname[len(prefix) :], case.get("name"))] += 1
                output = "".join(case.itertext())
                if not re.search(r"test result: ok\. 1 passed; 0 failed;", output):
                    raise RuntimeError(
                        "Rust JUnit did not prove one Rust test executed"
                    )
                break
    if not expected or actual != expected:
        raise RuntimeError(
            f"Rust manifest/JUnit mismatch: {actual - expected}, {expected - actual}"
        )
    return {"tests": len(cases), "rust_passed": sum(actual.values()), "skipped": 0}


class Validation:
    def __init__(self, source, reports):
        self.source = Path(source).resolve()
        self.reports = Path(reports).resolve()
        self.reports.mkdir(mode=0o700, parents=True, exist_ok=True)
        self.scratch = Path(tempfile.mkdtemp(prefix="cloud-functional-validation-"))
        (self.scratch / "validation_observer.py").write_text(PLUGIN)
        self.results = {}
        self.interrupted = False
        self.env = dict(os.environ)
        self.env.update(
            LIBVIRT_DEFAULT_URI="qemu:///system",
            LC_ALL="C",
            PYTEST_ADDOPTS="",
            PYTHONDONTWRITEBYTECODE="1",
        )
        self.env["PATH"] = (
            f"{Path.home()}/.local/bin:{Path.home()}/.cargo/bin:" + self.env["PATH"]
        )
        for key in (
            "TRIDENT_FUNCTIONAL_BUILD_ID",
            "TRIDENT_FUNCTIONAL_CACHE_DIR",
            "TRIDENT_FUNCTIONAL_NETWORK",
            "PYTHONPATH",
            "PYTEST_PLUGINS",
        ):
            self.env.pop(key, None)

    def save(self, name, value):
        text = json.dumps(sanitized(value), indent=2, sort_keys=True)
        (self.reports / name).write_text(text + "\n")

    def run(self, name, args, expected=0, env=None, diagnostic=False):
        started = time.monotonic()
        tail = ""
        written = 0
        image_path = None
        with (self.reports / f"{name}.log").open("w") as log:
            log.write(redact(repr(args)) + "\n")
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
                    match = re.search(r"--functional-base-image (\S+)", safe)
                    if match:
                        image_path = match[1]
                    tail = (tail + safe)[-256 * 1024 :]
                    if written < MAX_LOG:
                        chunk = safe[: MAX_LOG - written]
                        log.write(chunk)
                        log.flush()
                        written += len(chunk)
                code = process.wait()
            if written >= MAX_LOG:
                log.write("\n[LOG BOUNDED; final output follows]\n" + tail)
        self.results[name] = {
            "exit": code,
            "expected": expected,
            "seconds": round(time.monotonic() - started, 1),
            "log_bounded": written >= MAX_LOG,
            "functional_base_image": image_path,
        }
        self.save("results.json", self.results)
        print(f"{name}: exit {code}, expected {expected}", flush=True)
        if code != expected or (self.interrupted and not diagnostic):
            raise RuntimeError(f"{name}: exit {code}, interrupted={self.interrupted}")
        return tail

    def inventory(self):
        return json.loads(
            subprocess.check_output(
                ["sudo", "python3", "-c", INVENTORY], text=True, stderr=subprocess.PIPE
            )
        )

    def lock_identity(self):
        return subprocess.check_output(
            ["sudo", "stat", "-Lc", "%d:%i:%h:%u:%g:%a:%s", LOCK], text=True
        ).strip()

    def capture_junit(self, name, report, before):
        if not report.exists() or report.stat().st_mtime_ns == before:
            raise RuntimeError(f"{name}: no fresh JUnit")
        if report.stat().st_size > MAX_LOG:
            raise RuntimeError(f"{name}: JUnit exceeds evidence budget")
        root = ET.fromstring(report.read_text())
        # Redact XML fields, not serialized markup, to preserve valid XML.
        for element in root.iter():
            element.attrib.update(
                {key: redact(value) for key, value in element.attrib.items()}
            )
            if element.text:
                element.text = redact(element.text)
            if element.tail:
                element.tail = redact(element.tail)
        ET.ElementTree(root).write(self.reports / f"{name}.xml", encoding="utf-8")
        return root

    def functional(self, name, mode, inject=None):
        namespace = f"validation-{os.environ['GITHUB_RUN_ID']}-{os.environ['GITHUB_RUN_ATTEMPT']}-{name}"
        binary_report = self.scratch / f"{name}-binaries.json"
        marker = self.scratch / f"{name}-marker"
        env = dict(
            self.env,
            TRIDENT_FUNCTIONAL_NAMESPACE=namespace,
            PYTHONPATH=str(self.scratch),
            PYTEST_ADDOPTS="-p validation_observer",
            VALIDATION_BINARIES=str(binary_report),
            VALIDATION_MARKER=str(marker),
        )
        if inject:
            env.update(
                VALIDATION_INJECT=inject, PYTEST_ADDOPTS="-p validation_observer -x"
            )
        report = self.source / "target/trident_functional_tests.xml"
        before = report.stat().st_mtime_ns if report.exists() else None
        expected = {"failure": 1, "interrupt": 2}.get(inject, 0)
        try:
            self.run(
                name,
                ["bash", "scripts/cloud-agent/run-functional.sh", mode],
                expected=expected,
                env=env,
            )
        finally:
            try:
                root = self.capture_junit(name, report, before)
            finally:
                after = self.inventory()
                self.save(f"{name}-inventory.json", after)
                self.run(
                    f"{name}-namespace-clean",
                    [
                        "sudo",
                        "python3",
                        "scripts/cloud-agent/select_functional_network.py",
                        namespace,
                    ],
                    diagnostic=True,
                )
                if after != self.baseline_inventory:
                    raise RuntimeError(
                        f"{name}: inventory not restored after matching cleanup"
                    )
                if self.lock_identity() != self.baseline_lock:
                    raise RuntimeError(f"{name}: lock inode/metadata changed")
                if binary_report.exists():
                    self.save(
                        f"{name}-binaries.json",
                        {
                            "candidate": SOURCE,
                            "binaries": json.loads(binary_report.read_text()),
                        },
                    )
                if self.results.get(name, {}).get("functional_base_image"):
                    self.cache_provenance(name)
        manifest_path = self.source / "tests/functional_tests/ft.json"
        manifest = json.loads(manifest_path.read_text())
        self.save(f"{name}-manifest.json", manifest)
        binaries = json.loads(binary_report.read_text())
        if not binaries:
            raise RuntimeError("No candidate binaries observed")
        if inject:
            created = any(
                c.get("name") == "test_create_vm"
                and not any(
                    c.find(t) is not None for t in ("failure", "error", "skipped")
                )
                for c in root.iter("testcase")
            )
            if not created or not marker.exists() or marker.read_text() != inject:
                raise RuntimeError("Injection did not reach SSH on real VM")
            if (
                inject == "failure"
                and "VALIDATION_CONTROLLED_FAILURE"
                not in ET.tostring(root, encoding="unicode")
            ):
                raise RuntimeError("Controlled pytest failure missing")
            counts = {"injected": inject, "real_functional_suite_evidence": False}
        else:
            counts = verify_junit(root, manifest, mode)
            output = "".join(root.itertext())
            for binary in binaries:
                if f"as {binary['remote']} (" not in output:
                    raise RuntimeError(
                        f"Candidate upload not observed: {binary['remote']}"
                    )
        self.results[name]["junit"] = counts
        self.save("results.json", self.results)

    def cache_provenance(self, name):
        image_path = self.results[name].get("functional_base_image")
        if not image_path:
            raise RuntimeError("No actual immutable image path")
        image = Path(image_path)
        provenance = json.loads((image.parent / "provenance.json").read_text())
        self.save(f"{name}-provenance.json", provenance)
        build = provenance["build"]
        if (
            build["definitionId"] != 5067
            or build["sourceBranch"] != "refs/heads/main"
            or build["result"] != "succeeded"
            or build["status"] != "completed"
            or not re.fullmatch(r"[a-f0-9]{40}", build["sourceVersion"])
            or not re.fullmatch(r"[a-f0-9]{64}", provenance["image"]["sha256"])
            or provenance["image"]["size"] != image.stat().st_size
        ):
            raise RuntimeError("Invalid immutable image provenance")
        identity = (
            str(image),
            image.stat().st_ino,
            image.stat().st_size,
            image.stat().st_mtime_ns,
            build["id"],
            build["sourceVersion"],
            provenance["image"]["sha256"],
        )
        if name == "smoke":
            self.image_identity = identity
            self.env["TRIDENT_FUNCTIONAL_BUILD_ID"] = str(build["id"])
        elif identity != self.image_identity or not provenance["cacheHit"]:
            raise RuntimeError("Subsequent run did not reuse the same immutable cache")

    def validate_go(self):
        self.save(
            "go-exclusions.json",
            {
                "package": "tridenttools/storm/e2e/scenario",
                "test": GO_BASELINE_TEST,
                "skip_regex": f"^{GO_BASELINE_TEST}$",
                "status": "excluded/not validated",
                "full_go_suite_validated": False,
                "evidence": GO_BASELINE_LIMIT,
            },
        )
        self.run(
            "go-filtered",
            [
                "bash",
                "-ec",
                "cd tools; go generate pkg/rcp/tlscerts/certs.go; "
                "go generate pkg/tridentgrpc/grpc.go; "
                f"go test -mod=readonly -skip '^{GO_BASELINE_TEST}$' ./...; "
                "go test -mod=readonly -race ./pkg/virtdeploy/...",
            ],
        )

    def execute(self):
        if (
            os.environ.get("GITHUB_ACTIONS") != "true"
            or os.environ.get("EXCLUSIVE_DISPOSABLE_1ES") != "true"
        ):
            raise RuntimeError("Requires explicitly exclusive disposable 1ES job")
        self.save(
            "identity.json",
            {
                "candidate": SOURCE,
                "harness": os.environ["GITHUB_SHA"],
                "runner": os.environ.get("RUNNER_NAME"),
                "scope": "FUNCTIONAL smoke/full, not full E2E slice C",
                "credential_resource": "499b84ac-1321-427f-aa17-267ca6975798",
                "no_guarantee": "SIGKILL cleanup or universal shared-host protection",
                "injection_path": "pinned source cargo build JSON -> upload_test_binaries(force_upload=True) -> ~/tests/<crate> -> RunnerTool --exact",
                "artifact_descriptor": json.loads(
                    (
                        self.source / "scripts/cloud-agent/functional-artifact.json"
                    ).read_text()
                ),
            },
        )
        if self.run("source-identity", ["git", "rev-parse", "HEAD"]).strip() != SOURCE:
            raise RuntimeError("Wrong pinned source")
        self.run(
            "prerequisite-identity",
            shlex.split(
                f"git diff --exit-code {PREREQ} HEAD -- "
                "tools tests/functional_tests Makefile"
            ),
        )
        if self.run("source-before", ["git", "status", "--porcelain"]).strip():
            raise RuntimeError("Source checkout is not clean")
        # An unavailable pre-install inventory is not an empty inventory.
        pre_inventory = None
        try:
            pre_inventory = self.inventory()
            self.save("inventory-pre-setup.json", pre_inventory)
        except subprocess.CalledProcessError as error:
            self.save(
                "inventory-pre-setup.json",
                {
                    "unavailable": redact(error.stderr),
                    "limitation": "No pre-install libvirt preservation claim",
                },
            )
        protected = [
            Path.home() / ".config/libvirt/libvirt.conf",
            self.source / "tools/vm-netlaunch.yaml",
            self.source / ".pytest_cache",
            self.source / "tests/functional_tests/.pytest_cache",
        ]
        protected += list((Path.home() / ".ssh").glob("*"))
        expected_files = snapshot(protected)
        self.save("files-original.json", expected_files)
        try:
            for iteration in (1, 2):
                self.run(
                    f"source-setup-{iteration}",
                    ["bash", "scripts/cloud-agent/setup.sh"],
                )
                gopath = subprocess.check_output(
                    ["go", "env", "GOPATH"], text=True
                ).strip()
                self.env["PATH"] = gopath + "/bin:" + self.env["PATH"]
                self.run(
                    f"functional-setup-{iteration}",
                    ["bash", "scripts/cloud-agent/setup-functional.sh"],
                )
                if snapshot(protected) != expected_files:
                    raise RuntimeError("Setup changed existing config/keys/cache")
                current = self.inventory()
                self.save(f"inventory-setup-{iteration}.json", current)
                lock = self.lock_identity()
                if iteration == 1:
                    if pre_inventory is not None and any(
                        current[kind].get(name) != entry
                        for kind, entries in pre_inventory.items()
                        for name, entry in entries.items()
                    ):
                        raise RuntimeError("Setup changed pre-existing inventory")
                    if (
                        current["domains"]
                        or current["pools"]
                        or set(current["networks"]) - {"default"}
                    ):
                        raise RuntimeError("Host is not fresh/exclusive")
                    self.baseline_inventory, self.baseline_lock = current, lock
                    protected += list((Path.home() / ".ssh").glob("*"))
                    protected.append(self.source / ".cargo/config")
                    expected_files = snapshot(protected)
                elif current != self.baseline_inventory or lock != self.baseline_lock:
                    raise RuntimeError("Repeated setup changed inventory/lock")
            self.save("files-before-runs.json", expected_files)
            checks = {
                "source-tests": "python3 -m unittest discover -s scripts/cloud-agent/tests -v",
                "fixture-tests": (
                    "python3 -m pytest tests/functional_tests/test_fixtures.py "
                    "-v -p no:cacheprovider"
                ),
                "privileged-lock-tests": (
                    "sudo python3 -m unittest discover -s scripts/cloud-agent/tests "
                    "-p test_run_functional.py -v"
                ),
                "format": "bash scripts/cloud-agent/validate.sh format",
            }
            for name, command in checks.items():
                output = self.run(name, shlex.split(command))
                if name == "source-tests":
                    skips = [
                        line for line in output.splitlines() if "... skipped" in line
                    ]
                    if any(
                        "test_provisioned_lock_reopens_across_uids_with_protected_regular"
                        not in line
                        for line in skips
                    ):
                        raise RuntimeError("Unexpected source test skip")
                    if not re.search(r"Ran 51 tests\b", output):
                        raise RuntimeError("Pinned source test count changed")
                if (
                    name in ("fixture-tests", "privileged-lock-tests")
                    and "skipped" in output.lower()
                ):
                    raise RuntimeError(f"{name}: unexpected skipped test")
                if name == "fixture-tests" and not re.search(r"\b16 passed\b", output):
                    raise RuntimeError("Did not execute all pinned fixture unit cases")
                if name == "privileged-lock-tests" and not re.search(
                    r"Ran 5 tests\b", output
                ):
                    raise RuntimeError("Did not execute privileged runner tests")
            self.validate_go()
            for name, mode, inject in (
                ("smoke", "smoke", None),
                ("full", "full", None),
                ("controlled-failure", "smoke", "failure"),
                ("graceful-interruption", "smoke", "interrupt"),
            ):
                self.functional(name, mode, inject)
        finally:
            after = snapshot(protected)
            self.save("files-after.json", after)
            status = self.run(
                "source-after", ["git", "status", "--porcelain"], diagnostic=True
            )
            self.save(
                "preservation.json",
                {
                    "files_preserved": after == expected_files,
                    "source_clean": not status.strip(),
                    "lock": (
                        self.lock_identity() if hasattr(self, "baseline_lock") else None
                    ),
                    "inventory": (
                        self.inventory()
                        if hasattr(self, "baseline_inventory")
                        else None
                    ),
                },
            )
            if after != expected_files or status.strip():
                raise RuntimeError("Protected files or source cleanliness changed")


def main():
    validation = Validation(*sys.argv[1:])

    def interrupted(signum, frame):
        validation.interrupted = True
        print(
            f"Received signal {signum}; waiting for source cleanup within outer timeout",
            flush=True,
        )

    signal.signal(signal.SIGINT, interrupted)
    signal.signal(signal.SIGTERM, interrupted)
    passed = False
    try:
        validation.execute()
        passed = True
        return 0
    except Exception as error:
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
        validation.save(
            "summary.json",
            {
                "passed": passed,
                "scope": "FUNCTIONAL only, not full E2E slice C",
                "results": validation.results,
                "not_run": [name for name in PLANNED if name not in validation.results],
                "limits": [
                    GO_BASELINE_LIMIT,
                    "No SIGKILL cleanup guarantee",
                    "No live shared-host safety fixtures",
                    "Route collision and lock contention use existing source tests",
                    "No live second-UID cache download; UID selection uses source tests",
                ],
            },
        )


if __name__ == "__main__":
    sys.exit(main())
