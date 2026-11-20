import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location(
    "hardening_validation",
    Path(__file__).resolve().parents[1] / "hardening-validation.py",
)
validation = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(validation)


class HardeningValidationTests(unittest.TestCase):
    def runner(self, root):
        scratch = root / "scratch"
        scratch.mkdir()
        with patch.dict(
            os.environ, GITHUB_RUN_ID="1", GITHUB_RUN_ATTEMPT="1"
        ), patch.object(validation.tempfile, "mkdtemp", return_value=str(scratch)):
            return validation.Validation(root, root / "reports")

    def test_redacts_urls_credential_lines_and_jwts(self):
        unsafe = (
            "image https://storage.invalid/image?sig=example\n"
            "Authorization: Bearer example\n"
            "access_token=example\n"
            "eyJheader.eyJpayload.signature\n"
            "Ran 12 tests in 0.1s\n"
        )
        safe = validation.redact(unsafe)
        self.assertNotIn("example", safe)
        self.assertNotIn("eyJ", safe)
        self.assertNotIn("https://", safe)
        self.assertIn("Ran 12 tests", safe)

    def test_junit_counts_exclude_skips_failures_and_errors(self):
        text = """
        <testsuites><testsuite>
          <testcase name="test_create_vm"/>
          <testcase name="test_deployment"/>
          <testcase name="test_find_disks"
                    classname="tests.functional_tests.ft.json.osutils.blkid"/>
          <testcase name="skip"
                    classname="tests.functional_tests.ft.json.osutils.blkid"><skipped/></testcase>
          <testcase name="fail"><failure/></testcase>
          <testcase name="error"><error/></testcase>
        </testsuite></testsuites>
        """
        counts = validation.junit_summary(text)
        self.assertEqual(counts["tests"], 6)
        self.assertEqual(counts["passed"], 3)
        self.assertEqual(counts["rust_functional_passed"], 1)
        for field in ("skipped", "failure", "error"):
            self.assertEqual(counts[field], 1)
        self.assertTrue(counts["vm_created"])
        self.assertTrue(counts["ssh_deployment"])

    def test_unrelated_junit_identities_do_not_count_as_rust(self):
        counts = validation.junit_summary(
            '<testsuite><testcase name="one" classname="functional.functest.helpers"/>'
            '<testcase name="two" classname="tests.functional_tests.ft.jsonish.osutils"/>'
            '<testcase name="three" classname="other.tests.functional_tests.ft.json.osutils"/>'
            '<testcase name="four" classname="tests.functional_tests.test_setup" '
            'file="tests/functional_tests/functest.py"/>'
            '<testcase name="five" classname="tests.functional_tests.ft.json"/>'
            "</testsuite>"
        )
        self.assertEqual(counts["passed"], 5)
        self.assertEqual(counts["rust_functional_passed"], 0)

    def test_lock_identity_uses_sudo_without_parent_group_access(self):
        with patch.object(
            Path, "stat", side_effect=PermissionError("libvirt group not refreshed")
        ), patch.object(
            Path, "exists", side_effect=PermissionError("libvirt group not refreshed")
        ), patch.object(
            validation.subprocess, "check_output", return_value="[2049, 12345]\n"
        ) as command:
            self.assertEqual(validation.lock_identity(), [2049, 12345])
        args = command.call_args.args[0]
        self.assertEqual(args[:3], ["sudo", "python3", "-c"])
        self.assertEqual(args[-1], str(validation.LOCK))
        compile(args[3], "lock_identity.py", "exec")

    def test_privileged_lock_identity_handles_missing_file(self):
        with tempfile.TemporaryDirectory() as temp:
            missing = Path(temp) / "missing.lock"

            def privileged(args, **kwargs):
                self.assertEqual(args[:3], ["sudo", "python3", "-c"])
                return subprocess.run(
                    [sys.executable, "-c", args[3], args[4]],
                    check=True,
                    capture_output=True,
                    text=True,
                ).stdout

            with patch.object(validation, "LOCK", missing), patch.object(
                validation.subprocess, "check_output", side_effect=privileged
            ):
                self.assertIsNone(validation.lock_identity())
                missing.touch()
                identity = missing.stat()
                self.assertEqual(
                    validation.lock_identity(), [identity.st_dev, identity.st_ino]
                )

    def test_privileged_lock_inspection_does_not_hide_errors(self):
        error = subprocess.CalledProcessError(1, ["sudo", "python3"])
        with patch.object(
            validation.subprocess, "check_output", side_effect=error
        ), self.assertRaises(subprocess.CalledProcessError):
            validation.lock_identity()

    def test_skipped_vm_is_not_execution_evidence(self):
        counts = validation.junit_summary(
            '<testsuite><testcase name="test_create_vm"><skipped/>'
            '</testcase><testcase name="test_deployment"><failure/>'
            "</testcase></testsuite>"
        )
        self.assertFalse(counts["vm_created"])
        self.assertFalse(counts["ssh_deployment"])
        self.assertEqual(counts["passed"], 0)

    def test_redacted_junit_remains_parseable(self):
        text = (
            '<testsuite><testcase name="case"><failure>'
            "download https://storage.invalid/file?sig=example&amp;other=value"
            "</failure></testcase></testsuite>"
        )
        self.assertEqual(
            validation.junit_summary(validation.redact_junit(text))["failure"], 1
        )

    def test_junit_credentials_cannot_remove_xml_structure(self):
        text = (
            '<testsuite><testcase name="case"><failure>'
            "Authorization: Bearer placeholder"
            "</failure></testcase></testsuite>"
        )
        safe = validation.redact_junit(text)
        self.assertNotIn("placeholder", safe)
        self.assertEqual(validation.junit_summary(safe)["failure"], 1)

    def test_json_redaction_preserves_values_and_structure(self):
        value = {"failure": {"message": "Authorization: Bearer placeholder"}}
        self.assertEqual(
            validation.sanitized(value),
            {"failure": {"message": "[REDACTED_CREDENTIAL_LINE]"}},
        )

    def test_snapshot_detects_content_change_and_preserves_missing_paths(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            file = root / "existing"
            file.write_text("before")
            before = validation.snapshot([root, root / "absent"])
            file.write_text("after")
            self.assertNotEqual(before, validation.snapshot([root, root / "absent"]))
            self.assertIsNone(before[str(root / "absent")])

    def test_injection_plugin_syntax(self):
        compile(validation.PLUGIN, "hardening_inject.py", "exec")
        self.assertIn('item.funcargs["vm"].execute("true")', validation.PLUGIN)

    def test_only_fresh_report_can_prove_functional_success(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / "target").mkdir()
            (root / "target/trident_functional_tests.xml").write_text("<testsuite/>")
            runner = self.runner(root)
            with patch.object(runner, "run", return_value=""), patch.object(
                runner, "assert_clean"
            ):
                with self.assertRaisesRegex(RuntimeError, "no fresh JUnit"):
                    runner.functional("smoke", "smoke")

    def test_nonzero_command_persists_redacted_failure_evidence(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            runner = self.runner(root)
            with self.assertRaisesRegex(RuntimeError, "exit 3"):
                runner.run(
                    "failure",
                    [
                        sys.executable,
                        "-c",
                        "print('https://invalid/?sig=placeholder');exit(3)",
                    ],
                )
            result = json.loads((root / "reports/results.json").read_text())
            self.assertEqual(result["failure"]["exit"], 3)
            self.assertTrue((root / "reports/first-failure.json").is_file())
            self.assertNotIn("placeholder", (root / "reports/failure.log").read_text())

    def test_fresh_skip_only_suite_cannot_pass_functional_gate(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / "target").mkdir()
            runner = self.runner(root)

            def run(*args, **kwargs):
                (root / "target/trident_functional_tests.xml").write_text(
                    '<testsuite><testcase name="test_create_vm"><skipped/>'
                    "</testcase></testsuite>"
                )
                return ""

            with patch.object(runner, "run", side_effect=run), patch.object(
                runner, "assert_clean"
            ):
                with self.assertRaisesRegex(RuntimeError, "real VM/SSH/Rust"):
                    runner.functional("smoke", "smoke")
            self.assertTrue((root / "reports/smoke.xml").is_file())

    def test_fresh_vm_ssh_and_rust_results_pass_functional_gate(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / "target").mkdir()
            runner = self.runner(root)

            def run(*args, **kwargs):
                (root / "target/trident_functional_tests.xml").write_text(
                    '<testsuite><testcase name="test_create_vm"/>'
                    '<testcase name="test_deployment"/>'
                    '<testcase name="test_find_disks" '
                    'classname="tests.functional_tests.ft.json.osutils.blkid"/>'
                    "</testsuite>"
                )
                return "real execution"

            with patch.object(runner, "run", side_effect=run), patch.object(
                runner, "assert_clean"
            ):
                self.assertEqual(runner.functional("smoke", "smoke"), "real execution")
            self.assertEqual(runner.results["smoke"]["junit"]["passed"], 3)

    def test_vm_and_ssh_without_rust_cannot_pass_functional_gate(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / "target").mkdir()
            runner = self.runner(root)

            def run(*args, **kwargs):
                (root / "target/trident_functional_tests.xml").write_text(
                    '<testsuite><testcase name="test_create_vm"/>'
                    '<testcase name="test_deployment"/>'
                    '<testcase name="unrelated" classname="functional.functest.helpers"/>'
                    "</testsuite>"
                )
                return ""

            with patch.object(runner, "run", side_effect=run), patch.object(
                runner, "assert_clean"
            ):
                with self.assertRaisesRegex(RuntimeError, "real VM/SSH/Rust"):
                    runner.functional("smoke", "smoke")


if __name__ == "__main__":
    unittest.main()
