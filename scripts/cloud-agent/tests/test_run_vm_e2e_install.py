import argparse
import importlib.util
import json
import os
import signal
import subprocess
import tempfile
import unittest

from contextlib import nullcontext
from pathlib import Path
from unittest.mock import patch

RUNNER = Path(__file__).resolve().parents[1] / "run-vm-e2e-install.py"
SPEC = importlib.util.spec_from_file_location("vm_install", RUNNER)
runner = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(runner)
REVISION = "a" * 40


def matrix_entry(config, installer="trident-installer"):
    name = f"{config}_vm-host"
    return name, {
        "SCENARIO": name,
        "HARDWARE": "vm",
        "RUNTIME": "host",
        "TEST_RING": "ci",
        "tridentConfigurationName": config,
        "tridentDeploymentEnv": "virtualMachine",
        "tridentRuntimeEnv": "host",
        "installerISOName": installer,
    }


class InstallRunnerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.bundle = self.root / "bundle with spaces"
        self.bundle.mkdir()
        paths = {
            "bin/storm-trident": "go-tools",
            "artifacts/iso/trident-installer.iso": "trident-installer",
            "artifacts/iso/trident-split-installer.iso": "trident-split-installer",
            "artifacts/test-image/trident-testimage.cosi": "trident-testimage",
            "artifacts/test-image/trident-testimage_v2.cosi": "trident-testimage",
            "artifacts/usrverity/ca_cert.pem": "trident-usrverity-testimage",
        }
        files = {}
        for name, artifact in paths.items():
            path = self.bundle / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(b"mock artifact, never execute or boot")
            files[name] = {
                "artifact": artifact,
                "size": path.stat().st_size,
                "sha256": runner.digest(path),
            }
        (self.bundle / "bin/storm-trident").chmod(0o755)
        self.manifest = {
            "schemaVersion": 1,
            "candidateRevision": REVISION,
            "producer": {
                "organization": "mariner-org",
                "projectId": runner.PROJECT_ID,
                "definitionId": 5073,
                "buildId": 123,
                "sourceVersion": REVISION,
            },
            "files": files,
            "signingCertificate": "artifacts/usrverity/ca_cert.pem",
        }
        self.manifest_path = self.bundle / "provenance.json"
        self.args = argparse.Namespace(
            bundle=self.bundle,
            manifest=self.manifest_path,
            manifest_sha256="",
            expected_revision=REVISION,
            output=self.root / "output with spaces",
            stage_type="ci",
            scenario=None,
            timeout_seconds=120,
            cleanup_grace_seconds=30,
        )
        self.save_manifest()
        self.matrix = dict(
            [matrix_entry("base"), matrix_entry("split", "trident-split-installer")]
        )
        self.commands = []
        self.failure = 0
        self.report = '<testsuites><testsuite><testcase name="install" status="PASS"/></testsuite></testsuites>'

    def save_manifest(self):
        self.manifest_path.write_text(json.dumps(self.manifest))
        self.args.manifest_sha256 = runner.digest(self.manifest_path)

    def mock_command(self, argv, cwd, log, timeout, grace):
        self.commands.append((argv, cwd, timeout, grace))
        if argv[1] == "script":
            log.write_text(runner.MATRIX_PREFIX + json.dumps(self.matrix) + "\n")
            return 0
        log.write_text("mock Storm console retained\n")
        Path(argv[argv.index("-j") + 1]).write_text(self.report)
        (cwd / "serial.log").write_text("mock serial retained\n")
        return self.failure

    def run_mock(self, host_error=None):
        with (
            patch.object(runner, "host_lock", return_value=nullcontext()),
            patch.object(runner, "check_host", side_effect=host_error),
            patch.object(runner, "command", side_effect=self.mock_command),
        ):
            status = runner.execute(self.args)
        return status, json.loads((self.args.output / "results.json").read_text())

    def test_pipeline_contract_sequential_matrix_and_retained_evidence(self):
        status, result = self.run_mock()
        self.assertEqual(status, 0)
        self.assertEqual(result["outcome"], "passed")
        self.assertEqual(result["provenance"], self.manifest)
        self.assertEqual(self.commands[0][0][1:], ["script", "e2e-matrix", "ci"])
        self.assertEqual(len(self.commands), 3)
        for index, config in enumerate(("base", "split"), 1):
            output = self.args.output / f"{config}_vm-host"
            iso = (
                "trident-split-installer" if config == "split" else "trident-installer"
            )
            self.assertEqual(
                self.commands[index],
                (
                    [
                        str(self.bundle / "bin/storm-trident"),
                        "run",
                        f"{config}_vm-host",
                        "-o",
                        str(output),
                        "-j",
                        str(output / "storm-e2e.junit.xml"),
                        "--",
                        "--pipeline-run",
                        "--iso",
                        str(self.bundle / f"artifacts/iso/{iso}.iso"),
                        "--test-image-dir",
                        str(output / "test-image"),
                        "--test-ring",
                        "ci",
                        "--logstream-file",
                        str(output / "logstream-full.log"),
                        "--tracestream-file",
                        str(output / "tracestream.jsonl"),
                        "--signing-cert",
                        str(self.bundle / self.manifest["signingCertificate"]),
                    ],
                    output,
                    120,
                    30,
                ),
            )
            self.assertTrue((output / "console.log").is_file())
            self.assertTrue((output / "serial.log").is_file())
            self.assertEqual(
                result["scenarios"][index - 1]["cases"],
                [{"name": "install", "outcome": "passed"}],
            )
        runner.verify_bundle(
            self.bundle, self.manifest_path, self.args.manifest_sha256, REVISION
        )

    def test_select_one_and_record_unselected_as_not_run(self):
        self.args.scenario = "split_vm-host"
        status, result = self.run_mock()
        self.assertEqual(status, 0)
        self.assertEqual(len(self.commands), 2)
        self.assertFalse(result["scenarios"][0]["selected"])
        self.assertEqual(result["scenarios"][0]["outcome"], "not_run")

    def test_preserve_first_failure_and_stop_matrix(self):
        self.failure = 23
        self.report = (
            '<testsuite><testcase name="install"><failure/></testcase></testsuite>'
        )
        status, result = self.run_mock()
        self.assertEqual(status, 23)
        self.assertEqual(
            [r["outcome"] for r in result["scenarios"]], ["failed", "not_run"]
        )
        self.assertEqual(len(self.commands), 2)

    def test_missing_junit_does_not_replace_primary_failure(self):
        self.failure = 19
        self.report = "not XML"
        status, result = self.run_mock()
        self.assertEqual(status, 19)
        self.assertIn("reportError", result["scenarios"][0])

    def test_success_without_real_pass_is_failure(self):
        for report in (
            "<testsuite/>",
            '<testsuite><testcase name="pending"><skipped type="NotRun"/></testcase></testsuite>',
            '<testsuite><testcase name="bad"><failure/></testcase></testsuite>',
            '<testsuite><testcase name="skip"><skipped type="Skipped"/></testcase></testsuite>',
            '<testsuite><testcase name="unknown" status="new"/></testsuite>',
        ):
            with self.subTest(report=report):
                self.args.output = self.root / str(len(list(self.root.iterdir())))
                self.report = report
                status, result = self.run_mock()
                self.assertNotEqual(status, 0)
                self.assertEqual(result["outcome"], "failed")

    def test_case_accounting_distinguishes_skip_and_not_run(self):
        junit = self.root / "junit.xml"
        junit.write_text(
            """<testsuite>
          <testcase name="pass" status="PASS"/>
          <testcase name="skip"><skipped type="Skipped"/></testcase>
          <testcase name="not-run"><skipped type="NotRun"/></testcase>
          <testcase name="pending"><skipped type="Pending"/></testcase>
          <testcase name="error"><error/></testcase>
        </testsuite>"""
        )
        self.assertEqual(
            [r["outcome"] for r in runner.case_results(junit)],
            ["passed", "skipped", "not_run", "not_run", "failed"],
        )

    def test_bundle_rejections_precede_any_command(self):
        mutations = (
            lambda: self.manifest["producer"].update(sourceVersion="b" * 40),
            lambda: self.manifest.update(candidateRevision="b" * 40),
            lambda: self.manifest["producer"].update(buildId="latest"),
            lambda: self.manifest["producer"].update(projectId="other-project"),
            lambda: self.manifest["files"]["bin/storm-trident"].update(sha256="0" * 64),
            lambda: self.manifest["files"]["bin/storm-trident"].update(size=1),
            lambda: self.manifest.update(signingCertificate="unverified.pem"),
        )
        original = json.dumps(self.manifest)
        for index, mutation in enumerate(mutations):
            with self.subTest(index=index):
                self.manifest = json.loads(original)
                mutation()
                self.save_manifest()
                self.args.output = self.root / f"reject-{index}"
                status, result = self.run_mock()
                self.assertNotEqual(status, 0)
                self.assertEqual(result["outcome"], "failed")
                self.assertEqual(self.commands, [])

    def test_manifest_pin_and_unlisted_files_fail(self):
        self.manifest_path.write_text("{}")
        with self.assertRaisesRegex(ValueError, "manifest hash mismatch"):
            runner.verify_bundle(
                self.bundle, self.manifest_path, self.args.manifest_sha256, REVISION
            )
        self.save_manifest()
        (self.bundle / "unlisted.cosi").write_bytes(b"old image")
        status, _ = self.run_mock()
        self.assertNotEqual(status, 0)
        self.assertEqual(self.commands, [])

    def test_missing_bundle_fails_without_commands(self):
        self.args.bundle = self.root / "missing"
        status, _ = self.run_mock()
        self.assertNotEqual(status, 0)
        self.assertEqual(self.commands, [])

    def test_matrix_missing_empty_malformed_unsupported_and_unknown_stage(self):
        for text in (
            "",
            runner.MATRIX_PREFIX + "{}",
            runner.MATRIX_PREFIX + "[]",
            runner.MATRIX_PREFIX + "{",
            runner.MATRIX_PREFIX + "{}\n" + runner.MATRIX_PREFIX + "{}",
            "Unknown test ring 'typo'\n"
            + runner.MATRIX_PREFIX
            + json.dumps(self.matrix),
            runner.MATRIX_PREFIX
            + json.dumps({"../escape": next(iter(self.matrix.values()))}),
            runner.MATRIX_PREFIX
            + json.dumps(dict([matrix_entry("base")])).replace('"vm"', '"bm"'),
        ):
            with self.subTest(text=text), self.assertRaises((ValueError, KeyError)):
                runner.parse_matrix(text)

    def test_missing_iso_or_registry_inputs_fail_before_scenarios(self):
        for config, installer in (
            ("base", "missing"),
            ("extensions", "trident-installer"),
        ):
            with self.subTest(config=config):
                self.args.output = self.root / config
                self.matrix = dict([matrix_entry(config, installer)])
                status, result = self.run_mock()
                self.assertNotEqual(status, 0)
                self.assertEqual(result["scenarios"][0]["outcome"], "not_run")
                self.assertEqual(len(self.commands), 1)
                self.commands.clear()

    def test_host_failure_is_not_green(self):
        status, result = self.run_mock(ValueError("KVM missing"))
        self.assertNotEqual(status, 0)
        self.assertEqual(result["error"], "KVM missing")
        self.assertEqual(self.commands, [])

    def test_output_must_be_new_and_outside_bundle(self):
        self.args.output = self.bundle / "output"
        with self.assertRaisesRegex(ValueError, "overlap"):
            runner.execute(self.args)
        self.args.output = self.root
        with self.assertRaisesRegex(ValueError, "overlap"):
            runner.execute(self.args)

    def test_unsafe_bundle_paths_and_symlink(self):
        for name in ("../escape", "/absolute", "a\\b"):
            with self.subTest(name=name), self.assertRaises(ValueError):
                runner.bundle_path(self.bundle, name)
        if os.name == "posix":
            (self.bundle / "link").symlink_to(self.manifest_path)
            with self.assertRaisesRegex(ValueError, "Symlinks"):
                runner.verify_bundle(
                    self.bundle, self.manifest_path, self.args.manifest_sha256, REVISION
                )

    @unittest.skipUnless(os.name == "posix", "requires POSIX process signals and locks")
    def test_timeout_signals_storm_then_bounds_cleanup(self):
        log = self.root / "timeout.log"
        with patch.object(runner.subprocess, "Popen") as popen:
            child = popen.return_value
            child.pid = 12345
            child.wait.side_effect = [
                subprocess.TimeoutExpired("mock-storm", 1),
                subprocess.TimeoutExpired("mock-storm", 2),
                -9,
            ]
            with patch.object(runner.os, "killpg") as kill:
                status = runner.command(["mock-storm"], self.root, log, 1, 2)
            self.assertEqual(status, 124)
            child.terminate.assert_called_once()
            kill.assert_called_once_with(12345, signal.SIGKILL)
            self.assertEqual(
                [c.kwargs for c in child.wait.call_args_list],
                [{"timeout": 1}, {"timeout": 2}, {}],
            )

    @unittest.skipUnless(os.name == "posix", "requires POSIX process signals and locks")
    def test_interruption_forwards_cancellation(self):
        with patch.object(runner.subprocess, "Popen") as popen:
            child = popen.return_value
            child.wait.side_effect = [KeyboardInterrupt(), 0]
            self.assertEqual(
                runner.command(["mock"], self.root, self.root / "log", 1, 2), 130
            )
            child.terminate.assert_called_once()

    @unittest.skipUnless(os.name == "posix", "requires POSIX process signals and locks")
    def test_repeated_cancellation_cannot_interrupt_cleanup_or_reaping(self):
        handlers = {
            sig: signal.getsignal(sig) for sig in (signal.SIGINT, signal.SIGTERM)
        }
        lock = self.root / "host.lock"
        lock.touch()

        def cancel_again(*args, **kwargs):
            signal.raise_signal(signal.SIGINT)
            signal.raise_signal(signal.SIGTERM)
            with self.assertRaises(BlockingIOError), runner.host_lock():
                self.fail("Cleanup released the host lock")
            if kwargs:
                raise subprocess.TimeoutExpired("mock-storm", 2)
            return -9

        for primary, expected in (
            (KeyboardInterrupt(), 130),
            (subprocess.TimeoutExpired("mock-storm", 1), 124),
        ):
            with (
                self.subTest(primary=type(primary).__name__),
                patch.object(runner, "HOST_LOCK", lock),
                patch.object(runner.subprocess, "Popen") as popen,
                patch.object(runner.os, "killpg") as kill,
                runner.host_lock(),
            ):
                child = popen.return_value
                child.pid = 12345
                waits = iter((primary, cancel_again, cancel_again))

                def wait(*args, **kwargs):
                    action = next(waits)
                    if isinstance(action, BaseException):
                        raise action
                    return action(*args, **kwargs)

                child.wait.side_effect = wait
                status = runner.command(
                    ["mock-storm"], self.root, self.root / "log", 1, 2
                )
                self.assertEqual(status, expected)
                child.terminate.assert_called_once()
                kill.assert_called_once_with(12345, signal.SIGKILL)
                self.assertEqual(
                    [c.kwargs for c in child.wait.call_args_list],
                    [{"timeout": 1}, {"timeout": 2}, {}],
                )
                self.assertEqual(
                    {sig: signal.getsignal(sig) for sig in handlers}, handlers
                )

    @unittest.skipUnless(os.name == "posix", "requires flock")
    def test_lock_excludes_other_suites_and_rejects_symlink(self):
        lock = self.root / "host.lock"
        lock.touch()
        with patch.object(runner, "HOST_LOCK", lock):
            with runner.host_lock():
                with self.assertRaises(BlockingIOError), runner.host_lock():
                    self.fail("Lock admitted concurrent suite")
            lock.unlink()
            lock.symlink_to(self.manifest_path)
            with self.assertRaises(OSError), runner.host_lock():
                self.fail("Lock admitted symlink")

    def test_timeout_configuration_rejects_unbounded_values(self):
        for value in ("0", "-1", "inf", "nan"):
            with (
                self.subTest(value=value),
                self.assertRaises(argparse.ArgumentTypeError),
            ):
                runner.positive_seconds(value)


if __name__ == "__main__":
    unittest.main()
