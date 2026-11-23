import argparse
import importlib.util
import json
import os
import pwd
import shutil
import signal
import stat
import struct
import subprocess
import sys
import tempfile
import unittest
import xml.etree.ElementTree as ET

from contextlib import contextmanager
from pathlib import Path
from unittest.mock import patch

RUNNER = Path(__file__).resolve().parents[1] / "run-vm-e2e-lifecycle.py"
SPEC = importlib.util.spec_from_file_location("vm_lifecycle", RUNNER)
runner = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(runner)
CLEANUP_SIGNALS = runner.shared.cleanup_signals
COMMAND = runner.shared.command
REVISION = "18dd6cd99d50a4268afd936acb1ee4d2b64881ea"
TOOL_REVISION = "b" * 40
PUBLIC = "ssh-rsa b3duZWQtbW9jay1wdWJsaWM= owned-mock-fixture\n"


class LifecycleRunnerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.bundle = self.root / "bundle with spaces"
        self.bundle.mkdir()
        self.key = self.root / "owned mock key"
        self.key.write_text("Non-cryptographic owned mock fixture\n")
        self.key.chmod(0o600)
        self.qemu = (
            pwd.getpwnam("nobody") if os.geteuid() == 0 else pwd.getpwuid(os.geteuid())
        )
        self.manifest_path = self.bundle / "provenance.json"
        self.manifest = {
            "schemaVersion": 1,
            "candidateRevision": REVISION,
            "producer": {
                "organization": "mariner-org",
                "projectId": runner.shared.PROJECT_ID,
                "definitionId": 5073,
                "buildId": 123,
                "sourceVersion": REVISION,
            },
            "files": {},
            "lifecycle": {
                "schemaVersion": 1,
                "review": {"reviewer": "mock-reviewer", "reference": "mock-review-1"},
                "stages": {
                    name: f"{index:08x}-2222-3333-4444-555555555555"
                    for index, name in enumerate(runner.STAGES, 1)
                },
                "imageCustomizerImage": (
                    "mcr.microsoft.com/azurelinux/imagecustomizer@sha256:" + "a" * 64
                ),
                "rollbackScriptSha256": runner.shared.digest(runner.SOURCE_SCRIPT),
                "toolRevision": TOOL_REVISION,
                "toolProducer": {
                    "organization": "mariner-org",
                    "projectId": runner.shared.PROJECT_ID,
                    "definitionId": 5073,
                    "buildId": 124,
                    "sourceVersion": TOOL_REVISION,
                },
            },
        }
        self.add_file("bin/storm-trident", "go-tools", b"mock executable")
        self.add_file("bin/netlisten", "go-tools", b"mock executable")
        self.add_file(
            runner.TOOL_PATH,
            "lifecycle-go-tools",
            b"mock separately built lifecycle tool",
        )
        tool_hash = self.manifest["files"][runner.TOOL_PATH]["sha256"]
        self.manifest["lifecycle"]["toolSha256"] = tool_hash
        self.add_file("ssh/id_rsa.pub", "ssh-keys", PUBLIC.encode())
        for flavor in runner.IMAGES:
            for role in ("base", "update-a", "update-b"):
                self.add_file(
                    runner.image_path(flavor, role),
                    f"image-{flavor}-{role}",
                    f"mock-{flavor}-{role}".encode(),
                )
        self.args = argparse.Namespace(
            bundle=self.bundle,
            manifest=self.manifest_path,
            manifest_sha256="",
            expected_revision=REVISION,
            output=self.root / "output with spaces",
            ssh_private_key=self.key,
            exclusive_disposable_host=True,
            expected_hostname="owned-disposable-1es",
            lane=None,
            timeout_seconds=120,
            cleanup_grace_seconds=5,
            qemu_user=self.qemu.pw_name,
            expected_tool_revision=TOOL_REVISION,
            tool_sha256=tool_hash,
        )
        self.save_manifest()
        self.commands = []
        self.failure = 0
        self.cleanup_failure = 0
        self.report_transform = lambda report: report
        self.in_guard = False
        self.lock = self.root / "host.lock"
        self.lock.touch()

    def add_file(self, name, artifact, content):
        path = self.bundle / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(content)
        if name.startswith(("bin/", "lifecycle-bin/")):
            path.chmod(0o755)
        self.manifest["files"][name] = {
            "artifact": artifact,
            "size": len(content),
            "sha256": runner.shared.digest(path),
        }

    def save_manifest(self):
        self.manifest_path.write_text(json.dumps(self.manifest))
        self.args.manifest_sha256 = runner.shared.digest(self.manifest_path)

    def assert_locked(self):
        with self.assertRaises(BlockingIOError), runner.shared.host_lock():
            self.fail("Lifecycle released the exclusive lock")

    @contextmanager
    def guard(self):
        self.assert_locked()
        with CLEANUP_SIGNALS():
            self.in_guard = True
            try:
                yield
            finally:
                self.in_guard = False
                self.assert_locked()

    def report(self, suite, flavor):
        report = ET.Element("testsuite")
        for name in runner.CASES[suite]:
            case = ET.SubElement(report, "testcase", name=name, status="PASS")
            if (
                suite == "servicing"
                and name == "publish-sig-image"
                or suite == "rollback"
                and flavor == "qemu"
                and name == "skip-to-ab-rollback"
            ):
                ET.SubElement(case, "skipped", type="Skipped")
        return ET.tostring(self.report_transform(report), encoding="unicode")

    def mock_command(self, argv, cwd, log, timeout, grace):
        self.assert_locked()
        self.commands.append((argv, cwd, timeout, grace))
        log.write_text("owned mock console\n")
        if "--help" in argv:
            log.write_text("--image-customizer-container-name\n")
            return 0
        if argv[0] == "setpriv":
            return 0
        if argv[0] == "ssh-keygen":
            log.write_text(PUBLIC)
            return 0
        if argv[0] == "docker":
            if argv[1:3] == ["container", "ls"]:
                log.write_text("")
                return 0
            log.write_text(
                json.dumps([self.manifest["lifecycle"]["imageCustomizerImage"]])
            )
            return 0
        if argv[0] == "virsh":
            if self.in_guard:
                self.assertEqual(log.name, "domain-after.log")
            log.write_text("error: Domain not found\n")
            return 1
        if argv[1] == "script":
            intent = json.loads((cwd.parent / "container-intent.json").read_text())
            self.assertFalse(intent["createMayHaveBeenAccepted"])
            for index in range(1, 4):
                (cwd / f"test-sysext-{index}.raw").write_text(f"mock sysext {index}")
            return 0
        if "--test-case-to-run" in argv:
            self.assertTrue(self.in_guard)
            self.assertEqual(argv[argv.index("--test-case-to-run") + 1], "cleanup-vm")
            self.assertTrue((cwd / "artifacts/booted.qcow2").is_file())
            return self.cleanup_failure
        self.assertFalse(self.in_guard)
        suite, flavor = cwd.name.split("-")
        if cwd.name == "rollback-uki":
            intent = json.loads((cwd / "container-intent.json").read_text())
            self.assertTrue(intent["createMayHaveBeenAccepted"])
            self.assertEqual(
                argv[argv.index("--image-customizer-container-name") + 1],
                intent["name"],
            )
        self.assertEqual(argv[2], suite)
        artifacts = cwd / "artifacts"
        base = artifacts / (runner.IMAGES[flavor] + ".qcow2")
        self.assertEqual(base.read_bytes(), f"mock-{flavor}-base".encode())
        # A suite owns one mutable disk/state chain. It is not reinstalled per case.
        base.write_bytes(b"mock customized base")
        (artifacts / "booted.qcow2").write_bytes(b"mock retained suite state")
        if suite == "servicing":
            self.assertEqual(
                (
                    artifacts / "update-a" / (runner.IMAGES[flavor] + ".cosi")
                ).read_bytes(),
                f"mock-{flavor}-update-a".encode(),
            )
            self.assertTrue((cwd / runner.SCRIPT).is_file())
        else:
            self.assertFalse((artifacts / "update-a").exists())
            self.assertTrue((artifacts / (runner.IMAGES[flavor] + ".cosi")).is_file())
        Path(argv[argv.index("-j") + 1]).write_text(self.report(suite, flavor))
        return self.failure

    def run_mock(self, command=None, guard=True):
        with (
            patch.object(runner.shared, "HOST_LOCK", self.lock),
            patch.object(
                runner.shared,
                "cleanup_signals",
                self.guard if guard else None,
                create=True,
            ),
            patch.object(runner, "check_host"),
            patch.object(runner, "check_ports"),
            patch.object(
                runner.shared, "command", side_effect=command or self.mock_command
            ),
        ):
            status = runner.execute(self.args)
        return status, json.loads((self.args.output / "results.json").read_text())

    def main_commands(self):
        return [
            item
            for item in self.commands
            if item[0][1] == "run"
            and "--test-case-to-run" not in item[0]
            and "--help" not in item[0]
        ]

    def test_all_enabled_lanes_sequential_isolated_and_pipeline_flags(self):
        status, result = self.run_mock()
        self.assertEqual(status, 0)
        self.assertEqual(result["executed"], 4)
        self.assertEqual(result["selected"], 4)
        self.assertEqual(result["counts"]["passed"], 4)
        self.assertEqual(result["caseCounts"]["skipped"], 3)
        self.assertEqual([c[1].name for c in self.main_commands()], list(runner.LANES))
        for argv, work, timeout, grace in self.main_commands():
            suite, flavor = work.name.split("-")
            self.assertEqual((timeout, grace), (120, 5))
            self.assertEqual(argv[argv.index("--platform") + 1], "qemu")
            self.assertEqual("--secure-boot" in argv, flavor == "qemu")
            self.assertEqual("--uki" in argv, suite == "rollback")
            self.assertEqual(
                "--skip-extension-testing" in argv, work.name == "rollback-qemu"
            )
            self.assertTrue((work / "artifacts/booted.qcow2").is_file())
            if suite == "servicing":
                self.assertEqual(argv[argv.index("--retry-count") + 1], "3")
                self.assertIn("--rollback", argv)
            if work.name == "rollback-uki":
                self.assertEqual(len(result["lanes"][3]["extensions"]), 3)
                self.assertIn("--image-customizer-image", argv)
        # Mutable staging did not alter a single immutable cohort input.
        runner.shared.verify_bundle(
            self.bundle, self.manifest_path, self.args.manifest_sha256, REVISION
        )
        self.assertEqual(result["lanes"][-1]["outcome"], "not_run")
        self.assertFalse(result["lanes"][-1]["selected"])
        self.assertIn("includeQemuGrub=false", result["lanes"][-1]["reason"])
        self.assertEqual(result["provenance"], self.manifest)
        self.assertEqual(result["toolRevision"], TOOL_REVISION)
        self.assertNotEqual(result["toolRevision"], result["candidateRevision"])
        self.assertFalse(result["requiresHostDisposal"])
        self.assertEqual(result["lanes"][3]["container"]["outcome"], "clean")

    def test_select_one_and_do_not_count_other_lanes_as_executed(self):
        self.args.lane = "rollback-qemu"
        status, result = self.run_mock()
        self.assertEqual(status, 0)
        self.assertEqual(result["selected"], 1)
        self.assertEqual(result["executed"], 1)
        self.assertEqual(result["counts"]["not_run"], 4)
        self.assertFalse(any(c[0][0] == "docker" for c in self.commands))

    def test_first_suite_failure_survives_cleanup_failure_and_stops_lanes(self):
        self.failure = 23
        self.cleanup_failure = 17
        status, result = self.run_mock()
        self.assertEqual(status, 23)
        self.assertEqual(len(self.main_commands()), 1)
        self.assertEqual(result["lanes"][0]["cleanupExitCode"], 17)
        self.assertEqual(result["lanes"][0]["exitCode"], 23)
        self.assertEqual(result["counts"]["failed"], 1)
        self.assertEqual(result["counts"]["not_run"], 4)

    def test_successful_suite_cannot_hide_cleanup_failure(self):
        self.cleanup_failure = 17
        status, result = self.run_mock()
        self.assertEqual(status, 17)
        self.assertEqual(result["lanes"][0]["outcome"], "failed")

    def test_timeout_and_cancellation_keep_original_status_and_cleanup_under_lock(self):
        for failure in (124, 130):
            with self.subTest(failure=failure):
                self.args.output = self.root / str(failure)
                self.failure = failure
                status, result = self.run_mock()
                self.assertEqual(status, failure)
                self.assertEqual(result["lanes"][0]["cleanupExitCode"], 0)
                self.assertEqual(result["executed"], 1)
                self.commands.clear()

    def test_interruption_exception_still_finalizes_under_guard_and_lock(self):
        def interrupted(argv, cwd, log, timeout, grace):
            status = self.mock_command(argv, cwd, log, timeout, grace)
            if (
                argv[1] == "run"
                and "--test-case-to-run" not in argv
                and "--help" not in argv
            ):
                raise KeyboardInterrupt
            return status

        status, result = self.run_mock(command=interrupted)
        self.assertEqual(status, 130)
        self.assertEqual(result["lanes"][0]["cleanupExitCode"], 0)
        self.assertEqual(result["outcome"], "failed")

    @unittest.skipUnless(os.name == "posix", "requires POSIX signals and flock")
    def test_repeated_signals_during_owned_cleanup_are_deferred_under_lock(self):
        handlers = {
            sig: signal.getsignal(sig) for sig in (signal.SIGINT, signal.SIGTERM)
        }

        def cancel_again(argv, cwd, log, timeout, grace):
            if self.in_guard:
                signal.raise_signal(signal.SIGINT)
                signal.raise_signal(signal.SIGTERM)
                self.assert_locked()
            return self.mock_command(argv, cwd, log, timeout, grace)

        self.failure = 130
        status, result = self.run_mock(command=cancel_again)
        self.assertEqual(status, 130)
        self.assertEqual(result["lanes"][0]["cleanupExitCode"], 0)
        self.assertEqual({sig: signal.getsignal(sig) for sig in handlers}, handlers)

    def test_extension_preparation_failure_is_unavailable_not_suite_execution(self):
        self.args.lane = "rollback-uki"

        def failed(argv, cwd, log, timeout, grace):
            status = self.mock_command(argv, cwd, log, timeout, grace)
            return 31 if argv[1] == "script" else status

        status, result = self.run_mock(command=failed)
        self.assertEqual(status, 31)
        self.assertEqual(result["outcome"], "unavailable")
        self.assertEqual(result["executed"], 0)
        self.assertEqual(result["lanes"][3]["extensionExitCode"], 31)
        self.assertEqual(self.main_commands(), [])

    def test_identical_update_images_rejected_even_with_valid_inventory_hashes(self):
        source = self.bundle / runner.image_path("uki", "update-a")
        self.add_file(
            runner.image_path("uki", "update-b"),
            "image-uki-update-b",
            source.read_bytes(),
        )
        self.save_manifest()
        status, result = self.run_mock()
        self.assertEqual(status, 1)
        self.assertIn("Identical update A/B", result["error"])
        self.assertEqual(self.commands, [])

    def test_reports_require_actual_enabled_passes_not_collection_or_partial_success(
        self,
    ):
        def skipped(report):
            ET.SubElement(
                report.find("./testcase[@name='deploy-vm']"), "skipped", type="Skipped"
            )
            return report

        def missing(report):
            report.remove(report.find("./testcase[@name='deploy-vm']"))
            return report

        for transform in (skipped, missing, lambda report: ET.Element("testsuite")):
            self.report_transform = transform
            self.args.output = self.root / f"reports-{len(list(self.root.iterdir()))}"
            status, result = self.run_mock()
            self.assertNotEqual(status, 0)
            self.assertIn("reportError", result["lanes"][0])
            self.assertEqual(result["lanes"][0]["cleanupExitCode"], 0)
            self.assertEqual(result["executed"], 1)
            self.commands.clear()

    def test_provenance_family_rejections_precede_any_command(self):
        mutations = (
            lambda: self.manifest["producer"].update(sourceVersion="b" * 40),
            lambda: self.manifest["lifecycle"].update(schemaVersion=2),
            lambda: self.manifest["lifecycle"].update(toolRevision="dirty"),
            lambda: self.manifest["lifecycle"].update(toolRevision="c" * 40),
            lambda: self.manifest["lifecycle"].update(toolSha256="a" * 64),
            lambda: self.manifest["lifecycle"]["toolProducer"].update(
                sourceVersion="c" * 40
            ),
            lambda: self.manifest["lifecycle"]["toolProducer"].update(buildId=True),
            lambda: self.manifest["files"].pop(runner.TOOL_PATH),
            lambda: self.manifest["lifecycle"].update(stages={}),
            lambda: self.manifest["lifecycle"].update(
                imageCustomizerImage="imagecustomizer:latest"
            ),
            lambda: self.manifest["lifecycle"].update(rollbackScriptSha256="b" * 64),
            lambda: self.manifest["files"][runner.image_path("uki", "base")].update(
                artifact="wrong"
            ),
            lambda: self.manifest["files"].pop(runner.image_path("qemu", "update-b")),
        )
        original = json.dumps(self.manifest)
        for index, mutate in enumerate(mutations):
            with self.subTest(index=index):
                self.manifest = json.loads(original)
                mutate()
                self.save_manifest()
                self.args.output = self.root / f"rejection-{index}"
                status, result = self.run_mock()
                self.assertNotEqual(status, 0)
                self.assertEqual(result["counts"]["unavailable"], 4)
                self.assertEqual(result["executed"], 0)
                self.assertEqual(self.commands, [])

    def test_unintegrated_shared_guard_is_truthful_unavailable(self):
        status, result = self.run_mock(guard=False)
        self.assertNotEqual(status, 0)
        self.assertIn("Integration prerequisite", result["error"])
        self.assertEqual(result["outcome"], "unavailable")
        self.assertEqual(self.commands, [])

    def test_missing_private_key_fails_before_any_suite(self):
        self.args.ssh_private_key = self.root / "missing key"
        status, result = self.run_mock()
        self.assertNotEqual(status, 0)
        self.assertEqual(result["executed"], 0)
        self.assertEqual(self.commands, [])

    def test_wrong_key_or_missing_pinned_mic_never_starts_suite(self):
        for mode in ("wrong-key", "missing-mic"):

            def unavailable(argv, cwd, log, timeout, grace):
                status = self.mock_command(argv, cwd, log, timeout, grace)
                if mode == "wrong-key" and argv[0] == "ssh-keygen":
                    log.write_text("ssh-rsa b3RoZXItbW9jaw==\n")
                if mode == "missing-mic" and argv[0] == "docker":
                    return 19
                return status

            self.args.output = self.root / mode
            status, result = self.run_mock(command=unavailable)
            self.assertEqual(status, 19 if mode == "missing-mic" else 1)
            self.assertEqual(result["executed"], 0)
            self.assertEqual(self.main_commands(), [])
            self.commands.clear()

    def test_pinned_old_tool_without_container_hook_is_unavailable(self):
        def old_help(argv, cwd, log, timeout, grace):
            status = self.mock_command(argv, cwd, log, timeout, grace)
            if "--help" in argv:
                log.write_text("old rollback options\n")
            return status

        status, result = self.run_mock(command=old_help)
        self.assertEqual(status, 1)
        self.assertIn("lacks the owned-container hook", result["error"])
        self.assertEqual(result["executed"], 0)

    def test_existing_domain_is_not_deleted(self):
        def exists(argv, cwd, log, timeout, grace):
            status = self.mock_command(argv, cwd, log, timeout, grace)
            return 0 if argv[0] == "virsh" else status

        status, result = self.run_mock(command=exists)
        self.assertNotEqual(status, 0)
        self.assertEqual(result["executed"], 0)
        self.assertEqual(result["lanes"][0]["outcome"], "unavailable")
        self.assertFalse(any("--test-case-to-run" in c[0] for c in self.commands))

    def test_existing_container_is_not_deleted_or_used(self):
        self.args.lane = "rollback-uki"

        def exists(argv, cwd, log, timeout, grace):
            status = self.mock_command(argv, cwd, log, timeout, grace)
            if argv[0:3] == ["docker", "container", "ls"]:
                intent = json.loads((cwd / "container-intent.json").read_text())
                self.assertFalse(intent["createMayHaveBeenAccepted"])
                log.write_text("c" * 64 + "\n")
            return status

        status, result = self.run_mock(command=exists)
        self.assertEqual(status, 1)
        self.assertEqual(result["executed"], 0)
        self.assertIn("Container name already exists", result["lanes"][3]["error"])
        self.assertFalse(any(c[0][1] == "script" for c in self.commands))
        self.assertFalse(any("rm" in c[0] for c in self.commands))

    def test_output_must_be_new_and_not_overlap_bundle(self):
        for output in (self.bundle / "output", self.root):
            self.args.output = output
            with self.assertRaises(ValueError):
                runner.execute(self.args)
        self.args.output = self.root / "already exists"
        self.args.output.mkdir()
        with self.assertRaises(FileExistsError):
            runner.execute(self.args)

    def test_permissive_caller_umask_does_not_expose_evidence_or_staged_sources(self):
        previous = os.umask(0)
        try:
            status, _ = self.run_mock()
            restored = os.umask(0)
            self.assertEqual(restored, 0)
        finally:
            os.umask(previous)
        self.assertEqual(status, 0)
        self.assertEqual(stat.S_IMODE(self.args.output.stat().st_mode), 0o710)
        self.assertEqual(self.args.output.stat().st_gid, self.qemu.pw_gid)
        for path in (
            self.args.output / "results.json",
            self.args.output / "servicing-qemu/console.log",
            self.args.output
            / "servicing-qemu/artifacts/trident-vm-grub-verity-testimage.qcow2",
        ):
            self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o600, str(path))

    @unittest.skipUnless(sys.platform == "linux", "requires Linux POSIX ACL xattrs")
    def test_default_acl_inheritance_is_rejected_before_evidence_creation(self):
        parent = self.root / "default ACL parent"
        parent.mkdir()
        # A permissive inherited default ACL can override an otherwise private umask.
        acl = struct.pack("<I", 2) + b"".join(
            struct.pack("<HHI", tag, 7, 0xFFFFFFFF) for tag in (1, 4, 32)
        )
        os.setxattr(parent, "system.posix_acl_default", acl)
        original = parent.stat()
        self.args.output = parent / "output"
        with self.assertRaisesRegex(ValueError, "default ACL"):
            runner.execute(self.args)
        self.assertEqual(list(self.args.output.iterdir()), [])
        self.assertEqual(parent.stat().st_mode, original.st_mode)
        self.assertEqual(os.getxattr(parent, "system.posix_acl_default"), acl)

    @unittest.skipUnless(
        os.name == "posix" and os.geteuid() == 0 and shutil.which("setpriv"),
        "requires root and setpriv for real cross-UID validation",
    )
    def test_real_qemu_uid_opens_owned_disk_but_not_private_evidence(self):
        self.root.chmod(0o755)
        ancestor = self.root.stat()
        self.args.lane = "servicing-qemu"

        def commands(argv, cwd, log, timeout, grace):
            if argv[0] == "setpriv":
                self.assert_locked()
                return COMMAND(argv, cwd, log, timeout, grace)
            status = self.mock_command(argv, cwd, log, timeout, grace)
            if argv[1] == "run" and "--test-case-to-run" not in argv:
                # Model libvirt's existing DAC ownership change, without a VM.
                disk = cwd / "artifacts/booted.qcow2"
                os.chown(disk, self.qemu.pw_uid, self.qemu.pw_gid)
                self.probe_qemu(
                    """from pathlib import Path
import os, sys
with open(sys.argv[1], "r+b") as disk:
    assert disk.read() == b"mock retained suite state"
    disk.seek(0)
    disk.write(b"mock retained suite state")
for name in sys.argv[2:]:
    try:
        Path(name).read_bytes()
    except PermissionError:
        continue
    raise AssertionError("Private file became readable: " + name)
""",
                    disk,
                    cwd / "console.log",
                    cwd / "artifacts/trident-vm-grub-verity-testimage.qcow2",
                    self.key,
                )
            return status

        status, _ = self.run_mock(command=commands)
        self.assertEqual(status, 0)
        self.probe_qemu(
            """from pathlib import Path
import os, sys
try:
    Path(sys.argv[1]).read_bytes()
except PermissionError:
    pass
else:
    raise AssertionError("Evidence readable")
try:
    os.listdir(sys.argv[2])
except PermissionError:
    pass
else:
    raise AssertionError("Search-only output became listable")
""",
            self.args.output / "results.json",
            self.args.output,
        )
        current = self.root.stat()
        self.assertEqual(
            (current.st_mode, current.st_uid, current.st_gid),
            (ancestor.st_mode, ancestor.st_uid, ancestor.st_gid),
        )

    def probe_qemu(self, code, *paths):
        subprocess.run(
            [
                "setpriv",
                "--reuid",
                str(self.qemu.pw_uid),
                "--regid",
                str(self.qemu.pw_gid),
                "--clear-groups",
                sys.executable,
                "-c",
                code,
                *map(str, paths),
            ],
            check=True,
            capture_output=True,
            timeout=5,
        )

    @unittest.skipUnless(
        os.name == "posix" and os.geteuid() == 0 and shutil.which("setpriv"),
        "requires root and setpriv for real cross-UID validation",
    )
    def test_private_existing_ancestor_fails_without_permission_changes(self):
        self.root.chmod(0o755)
        private = self.root / "private ancestor"
        private.mkdir(mode=0o700)
        ancestor = private.stat()
        self.args.output = private / "new output"
        self.args.lane = "servicing-qemu"

        def commands(argv, cwd, log, timeout, grace):
            if argv[0] == "setpriv":
                return COMMAND(argv, cwd, log, timeout, grace)
            return self.mock_command(argv, cwd, log, timeout, grace)

        status, result = self.run_mock(command=commands)
        self.assertNotEqual(status, 0)
        self.assertEqual(result["executed"], 0)
        self.assertIn("already searchable parent", result["error"])
        current = private.stat()
        self.assertEqual(
            (current.st_mode, current.st_uid, current.st_gid),
            (ancestor.st_mode, ancestor.st_uid, ancestor.st_gid),
        )

    def test_explicit_hostname_acknowledgement_and_missing_tool_fail_closed(self):
        with (
            patch.object(runner.socket, "gethostname", return_value="other-host"),
            patch.object(runner.shared, "check_host") as check,
        ):
            with self.assertRaisesRegex(ValueError, "hostname mismatch"):
                runner.check_host(self.args)
            check.assert_not_called()
        with (
            patch.object(
                runner.socket, "gethostname", return_value=self.args.expected_hostname
            ),
            patch.object(runner.os, "geteuid", return_value=0),
            patch.object(runner.shared, "check_host"),
            patch.object(runner.shutil, "which", return_value=None),
        ):
            with self.assertRaisesRegex(ValueError, "virt-install"):
                runner.check_host(self.args)

    @unittest.skipUnless(os.name == "posix", "requires Linux child processes and flock")
    def test_real_mock_suite_process_and_named_cleanup(self):
        executable = b"""#!/usr/bin/env python3
import pathlib, sys
args = sys.argv
root = pathlib.Path.cwd()
report = pathlib.Path(args[args.index('-j') + 1])
if '--test-case-to-run' in args:
    (root / 'cleanup-invoked').write_text(args[args.index('--name') + 1])
    report.write_text('<testsuite><testcase name="cleanup-vm" status="PASS"/></testsuite>')
else:
    (root / 'artifacts/booted.qcow2').write_text('mock process disk')
    names = ('publish-sig-image', 'deploy-vm', 'check-deployment', 'update-loop', 'rollback', 'collect-logs', 'cleanup-vm')
    cases = ''.join('<testcase name="' + n + '" status="PASS">' + ('<skipped type="Skipped"/>' if n == 'publish-sig-image' else '') + '</testcase>' for n in names)
    report.write_text('<testsuite>' + cases + '</testsuite>')
"""
        self.add_file(runner.TOOL_PATH, "lifecycle-go-tools", executable)
        tool_hash = self.manifest["files"][runner.TOOL_PATH]["sha256"]
        self.manifest["lifecycle"]["toolSha256"] = tool_hash
        self.args.tool_sha256 = tool_hash
        self.save_manifest()
        self.args.lane = "servicing-qemu"
        with (
            patch.object(runner.shared, "HOST_LOCK", self.lock),
            patch.object(runner.shared, "cleanup_signals", self.guard, create=True),
            patch.object(runner, "preflight", return_value=(self.key, self.qemu)),
            patch.object(runner, "check_ports"),
            patch.object(runner, "domain_absent"),
        ):
            status = runner.execute(self.args)
        self.assertEqual(status, 0)
        work = self.args.output / "servicing-qemu"
        self.assertTrue((work / "artifacts/booted.qcow2").is_file())
        result = json.loads((self.args.output / "results.json").read_text())
        self.assertEqual(
            (work / "cleanup-invoked").read_text(), result["lanes"][0]["domain"]
        )
        self.assertEqual(result["caseCounts"]["passed"], 6)


if __name__ == "__main__":
    unittest.main()
