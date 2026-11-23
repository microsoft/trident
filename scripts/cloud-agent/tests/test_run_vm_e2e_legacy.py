import importlib.util
import json
import os
import signal
import subprocess
import sys
import tempfile
import unittest

from contextlib import nullcontext
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock, patch

import test_run_vm_e2e_install as fixtures

RUNNER = Path(__file__).resolve().parents[1] / "run-vm-e2e-legacy.py"
SPEC = importlib.util.spec_from_file_location("vm_legacy", RUNNER)
runner = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(runner)


class LegacyRunnerTests(unittest.TestCase):
    def setUp(self):
        self.fixture = fixtures.InstallRunnerTests()
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)
        self.args = self.fixture.args
        self.args.source = RUNNER.parents[2]
        self.args.suite = "legacy-vm-host-install-ab"
        self.args.build_purpose = "pullrequest"
        self.args.configuration = ["base"]
        self.bundle = self.fixture.bundle
        for name in (
            "bin/netlaunch",
            "bin/netlisten",
            "artifacts/test-image/regular.cosi",
            "artifacts/test-image/regular_v2.cosi",
            "artifacts/test-image/regular_v3.cosi",
            "artifacts/test-image/usrverity.cosi",
            "artifacts/test-image/usrverity_v2.cosi",
            "artifacts/test-image/usrverity_v3.cosi",
        ):
            path = self.bundle / name
            path.write_bytes(b"fixture only, never boot")
            path.chmod(0o755)
            self.fixture.manifest["files"][name] = {
                "artifact": (
                    "go-tools"
                    if name.startswith("bin/")
                    else (
                        "trident-usrverity-testimage"
                        if "usrverity" in name
                        else "trident-testimage"
                    )
                ),
                "size": path.stat().st_size,
                "sha256": runner.common.digest(path),
            }
        self.fixture.save_manifest()
        self.commands = []
        self.failure_label = None
        self.failure_code = 23
        self.cleanup_code = 0
        self.listener_status = 0
        self.listener_timeout = False
        self.report = (
            '<testsuite><testcase classname="base_test" name="test_state"/></testsuite>'
        )
        self.matrix = {
            name: {"configuration": name}
            for name in (
                "base",
                "simple",
                "split",
                "misc",
                "rerun",
                "health-checks-install",
            )
        }

    def command(self, args, argv, cwd, log):
        argv = [str(arg) for arg in argv]
        self.commands.append((log.stem, argv, cwd))
        log.write_text("fixture command, not VM runtime evidence\n")
        if log.stem == "matrix":
            log.write_text(runner.MATRIX_PREFIX + json.dumps(self.matrix) + "\n")
        if log.stem == "prepare-config":
            self.assertFalse(cwd.is_relative_to(self.args.output))
            Path(argv[argv.index("--keypath") + 1]).write_text("test-only placeholder")
        if log.stem == "create":
            (cwd / "tools/virt-deploy-metadata.json").write_text(
                json.dumps(
                    {"virtualmachines": [{"name": "fixture-vm", "ip": "192.0.2.1"}]}
                )
            )
        if "-m" in argv and "pytest" in argv:
            path = next(
                arg.split("=", 1)[1] for arg in argv if arg.startswith("--junit-xml=")
            )
            Path(path).write_text(self.report)
        if log.stem == "cleanup":
            return self.cleanup_code
        return self.failure_code if log.stem == self.failure_label else 0

    def run_mock(self, host_error=None):
        with (
            patch.object(runner.common, "host_lock", return_value=nullcontext()),
            patch.object(runner.common, "check_host", side_effect=host_error),
            patch.object(runner.shutil, "which", return_value="/mock/prerequisite"),
            patch.object(runner, "run_command", side_effect=self.command),
            patch.object(runner.subprocess, "Popen") as popen,
            patch.object(runner.common, "reap") as reap,
        ):
            popen.return_value.poll.return_value = None
            popen.return_value.wait.return_value = self.listener_status
            if self.listener_timeout:
                popen.return_value.wait.side_effect = subprocess.TimeoutExpired(
                    "fixture-netlisten", self.args.timeout_seconds
                )
            status = runner.execute(self.args)
        return (
            status,
            json.loads((self.args.output / "results.json").read_text()),
            popen,
            reap,
        )

    def selected(self, result):
        return [r for r in result["configurations"] if r["selected"]]

    def test_exact_legacy_sequence_state_and_no_mid_suite_cleanup(self):
        status, result, popen, reap = self.run_mock()
        self.assertEqual(status, 0)
        self.assertEqual(result["outcome"], "passed")
        row = self.selected(result)[0]
        self.assertEqual([p["name"] for p in row["phases"]], list(runner.PHASES))
        self.assertEqual(
            [p["activeVolumeAfter"] for p in row["phases"]],
            ["volume-a", "volume-a", "volume-b", "volume-b", "volume-a", "volume-a"],
        )
        self.assertTrue(
            all(p["executed"] and p["outcome"] == "passed" for p in row["phases"])
        )
        self.assertEqual(
            [label for label, _, _ in self.commands],
            [
                "dependencies",
                "matrix",
                "prepare-config",
                "key-pem",
                "uefi-fallback",
                "create",
                "netlaunch",
                "check-ssh",
                "check-tracing",
                "check-selinux",
                "pytest-a",
                "update-b",
                "pytest-b",
                "update-a",
                "pytest-a2",
                "cleanup",
                "cleanup-check",
            ],
        )
        commands = {label: argv for label, argv, _ in self.commands}
        self.assertIn("read_target_configurations.py", commands["matrix"][1])
        self.assertEqual(
            commands["matrix"][-6:],
            [
                "--runtimeEnv",
                "host",
                "--purpose",
                "pullrequest",
                "--matrix-name",
                "matrixConfigurations",
            ],
        )
        for label, volume in (
            ("pytest-a", "volume-a"),
            ("pytest-b", "volume-b"),
            ("pytest-a2", "volume-a"),
        ):
            cmd = commands[label]
            self.assertEqual(
                cmd[cmd.index("pytest") + 1 : cmd.index("pytest") + 3],
                ["-m", "pullrequest"],
            )
            self.assertEqual(cmd[-2:], ["--ab-active-volume", volume])
            self.assertNotIn("--collect-only", cmd)
        for label, version in (("update-b", "2"), ("update-a", "3")):
            cmd = commands[label]
            self.assertEqual(cmd[cmd.index("--version") + 1], version)
            self.assertEqual(cmd[-2:], ["--stage-ab-update", "--finalize-ab-update"])
        self.assertIn("--secure-boot", commands["netlaunch"])
        self.assertIn("--wait-for-provisioned-state", commands["netlaunch"])
        self.assertEqual(commands["create"][-4:], ["--mem", "12", "--disks", "32,32"])
        prefix = commands["create"][commands["create"].index("--nameprefix") + 1]
        self.assertRegex(prefix, r"^legacy-[0-9a-f]{32}-$")
        self.assertIn(prefix, commands["cleanup"])
        self.assertEqual(commands["cleanup"][-1], "--clean")
        self.assertEqual(popen.call_count, 2)
        self.assertEqual(reap.call_count, 2)
        reap.assert_called_with(popen.return_value, 30, group=True)
        self.assertEqual(
            row["phases"][1]["cases"],
            [{"name": "base_test::test_state", "outcome": "passed"}],
        )
        self.assertTrue(
            all(
                r["outcome"] == "not_run"
                for r in result["configurations"]
                if not r["selected"]
            )
        )
        self.assertFalse(any(self.args.output.rglob("key")))
        runner.common.verify_bundle(
            self.bundle,
            self.args.manifest,
            self.args.manifest_sha256,
            fixtures.REVISION,
        )

    def test_first_failure_stops_phases_and_preserves_cleanup_failure(self):
        self.failure_label = "update-b"
        self.cleanup_code = 9
        status, result, _, reap = self.run_mock()
        self.assertEqual(status, 23)
        row = self.selected(result)[0]
        self.assertEqual(row["cleanupExitCode"], 9)
        self.assertEqual(
            [p["outcome"] for p in row["phases"]],
            ["passed", "passed", "failed", "not_run", "not_run", "not_run"],
        )
        self.assertNotIn("activeVolumeAfter", row["phases"][2])
        reap.assert_called_once()
        self.assertEqual(
            [cmd[0] for cmd in self.commands[-2:]], ["cleanup", "cleanup-check"]
        )

    def test_cancellation_stops_before_updates_and_cleans(self):
        self.failure_label, self.failure_code = "pytest-a", 130
        status, result, popen, _ = self.run_mock()
        self.assertEqual(status, 130)
        self.assertEqual(
            [cmd[0] for cmd in self.commands[-2:]], ["cleanup", "cleanup-check"]
        )
        popen.assert_not_called()
        self.assertFalse(self.selected(result)[0]["phases"][2]["executed"])

    def test_cleanup_failure_prevents_pass(self):
        self.cleanup_code = 7
        status, result, _, _ = self.run_mock()
        self.assertEqual(status, 7)
        self.assertEqual(result["outcome"], "failed")

    def test_missing_image_version_blocks_all_vm_execution(self):
        name = "artifacts/test-image/regular_v3.cosi"
        (self.bundle / name).unlink()
        del self.fixture.manifest["files"][name]
        self.fixture.save_manifest()
        status, result, popen, _ = self.run_mock()
        self.assertNotEqual(status, 0)
        self.assertEqual(result["outcome"], "unavailable")
        self.assertIn(name, result["error"])
        self.assertEqual(self.selected(result)[0]["outcome"], "unavailable")
        self.assertFalse(any(label == "create" for label, _, _ in self.commands))
        popen.assert_not_called()

    def test_all_selected_inputs_checked_before_first_vm_registry_unavailable(self):
        self.args.configuration = ["base", "misc"]
        status, result, popen, _ = self.run_mock()
        self.assertNotEqual(status, 0)
        self.assertIn("authentication", result["error"])
        self.assertEqual(result["outcome"], "unavailable")
        self.assertEqual(
            [r["outcome"] for r in self.selected(result)], ["not_run", "unavailable"]
        )
        popen.assert_not_called()
        self.assertEqual(
            [label for label, _, _ in self.commands], ["dependencies", "matrix"]
        )

    def test_dependency_or_host_missing_is_unavailable(self):
        self.failure_label = "dependencies"
        status, result, _, _ = self.run_mock()
        self.assertEqual(status, 23)
        self.assertIn("prerequisites", result["error"])
        self.assertEqual(result["outcome"], "unavailable")
        self.args.output = self.fixture.root / "host-missing"
        status, result, _, _ = self.run_mock(ValueError("KVM missing"))
        self.assertNotEqual(status, 0)
        self.assertEqual(result["error"], "KVM missing")

    def test_provenance_tamper_blocks_before_any_command(self):
        (self.bundle / "bin/netlaunch").write_bytes(b"changed")
        status, result, _, _ = self.run_mock()
        self.assertNotEqual(status, 0)
        self.assertEqual(self.commands, [])
        self.assertEqual(result["outcome"], "unavailable")

    def test_report_failure_never_overwrites_primary_exit(self):
        self.failure_label, self.failure_code = "pytest-a", 19
        self.report = "not XML"
        status, result, _, _ = self.run_mock()
        self.assertEqual(status, 19)
        self.assertIn("reportError", self.selected(result)[0]["phases"][1])

    def test_empty_skipped_failed_or_malformed_reports_never_pass(self):
        for index, report in enumerate(
            (
                "<testsuite/>",
                "<testsuite><testcase name='s'><skipped/></testcase></testsuite>",
                "<testsuite><testcase name='f'><failure/></testcase></testsuite>",
                "<testsuite><testcase/></testsuite>",
                "not XML",
            )
        ):
            with self.subTest(report=report):
                self.args.output = self.fixture.root / f"report-{index}"
                self.report = report
                status, result, _, _ = self.run_mock()
                self.assertNotEqual(status, 0)
                self.assertEqual(result["outcome"], "failed")

    def test_sequential_sorted_selection_stops_after_first_failed_configuration(self):
        self.args.configuration = ["split", "base"]
        self.failure_label = "pytest-a"
        status, result, _, _ = self.run_mock()
        self.assertEqual(status, 23)
        self.assertEqual(
            [r["configuration"] for r in self.selected(result)], ["base", "split"]
        )
        self.assertEqual(
            [r["outcome"] for r in self.selected(result)], ["failed", "not_run"]
        )
        self.assertEqual(sum(label == "create" for label, _, _ in self.commands), 1)

    def test_non_ab_configuration_explicitly_skips_only_ab_phases(self):
        self.args.configuration = ["simple"]
        status, result, popen, _ = self.run_mock()
        self.assertEqual(status, 0)
        self.assertEqual(
            [p["outcome"] for p in self.selected(result)[0]["phases"]],
            ["passed", "passed", "skipped", "skipped", "skipped", "skipped"],
        )
        popen.assert_not_called()

    def test_special_netlaunch_and_expected_host_state_flags(self):
        for index, config in enumerate(("rerun", "health-checks-install")):
            with self.subTest(config=config):
                self.args.output = self.fixture.root / f"special-{index}"
                self.args.configuration = [config]
                self.commands = []
                status, _, _, _ = self.run_mock()
                self.assertEqual(status, 0)
                commands = {label: argv for label, argv, _ in self.commands}
                if config == "rerun":
                    self.assertNotIn("uefi-fallback", commands)
                    self.assertEqual(
                        commands["netlaunch"][-4:-2], ["--max-failures", "1"]
                    )
                else:
                    self.assertIn("--only-print-exit-code", commands["netlaunch"])
                    self.assertIn("--expect-failed-commit", commands["check-ssh"])
                    self.assertEqual(
                        commands["pytest-a"][-2:],
                        ["--expected-host-status-state", "not-provisioned"],
                    )

    def test_legacy_requests_group_graceful_cancellation(self):
        with patch.object(runner.common, "command", return_value=17) as command:
            self.assertEqual(
                runner.run_command(self.args, ["tool"], Path("/tmp"), Path("/tmp/log")),
                17,
            )
        command.assert_called_once_with(
            ["tool"], Path("/tmp"), Path("/tmp/log"), 120, 30, group_cleanup=True
        )

    def test_silent_virtdeploy_cleanup_failure_is_detected(self):
        self.failure_label = "cleanup-check"
        status, result, _, _ = self.run_mock()
        self.assertEqual(status, 23)
        row = self.selected(result)[0]
        self.assertEqual(row["cleanupExitCode"], 0)
        self.assertEqual(row["cleanupCheckExitCode"], 23)
        self.assertEqual(result["outcome"], "failed")

    def test_successful_multiple_configurations_are_sequential_with_split_iso(self):
        self.args.configuration = ["split", "base"]
        status, result, _, _ = self.run_mock()
        self.assertEqual(status, 0)
        rows = self.selected(result)
        self.assertEqual([r["configuration"] for r in rows], ["base", "split"])
        self.assertTrue(all(r["outcome"] == "passed" for r in rows))
        labels = [label for label, _, _ in self.commands]
        self.assertLess(
            labels.index("cleanup-check"),
            labels.index("create", labels.index("create") + 1),
        )
        installs = [argv for label, argv, _ in self.commands if label == "netlaunch"]
        self.assertIn(
            str(self.bundle / "artifacts/iso/trident-split-installer.iso"), installs[1]
        )

    def test_cleanup_repeated_signals_keep_host_lock_until_verified(self):
        lock = self.fixture.root / "host.lock"
        lock.touch()
        original_command = self.command

        def command(*args):
            if args[-1].stem in ("cleanup", "cleanup-check"):
                signal.raise_signal(signal.SIGINT)
                signal.raise_signal(signal.SIGTERM)
                with self.assertRaises(BlockingIOError), runner.common.host_lock():
                    self.fail("Host lock released during legacy cleanup")
            return original_command(*args)

        with (
            patch.object(runner.common, "HOST_LOCK", lock),
            patch.object(runner.common, "check_host"),
            patch.object(runner.shutil, "which", return_value="/mock/prerequisite"),
            patch.object(runner, "run_command", side_effect=command),
            patch.object(runner.subprocess, "Popen") as popen,
            patch.object(runner.common, "reap"),
        ):
            popen.return_value.poll.return_value = None
            popen.return_value.wait.return_value = 0
            self.assertEqual(runner.execute(self.args), 0)

    @unittest.skipUnless(os.name == "posix", "requires POSIX process groups")
    def test_one_shot_listener_exits_zero_during_b_then_fresh_listener_runs_a(self):
        processes = []
        launches = []
        real_popen = subprocess.Popen
        original_command = self.command

        def start_listener(argv, **kwargs):
            self.assertTrue(all(p.returncode == 0 for p in processes))
            process = real_popen(
                [sys.executable, "-c", "import sys; sys.stdin.buffer.read(1)"],
                stdin=subprocess.PIPE,
                **kwargs,
            )
            processes.append(process)
            launches.append(argv)
            return process

        def command(*args):
            if args[-1].stem in ("update-b", "update-a"):
                processes[-1].communicate(input=b"x", timeout=5)
                self.assertEqual(processes[-1].returncode, 0)
            return original_command(*args)

        try:
            with (
                patch.object(runner.common, "host_lock", return_value=nullcontext()),
                patch.object(runner.common, "check_host"),
                patch.object(runner.shutil, "which", return_value="/mock/prerequisite"),
                patch.object(runner, "run_command", side_effect=command),
                patch.object(runner.subprocess, "Popen", side_effect=start_listener),
            ):
                self.assertEqual(runner.execute(self.args), 0)
            self.assertEqual(len(processes), 2)
            self.assertIsNot(processes[0], processes[1])
            self.assertNotEqual(launches[0], launches[1])
            result = json.loads((self.args.output / "results.json").read_text())
            updates = self.selected(result)[0]["phases"][2::2]
            self.assertEqual([p["listenerExitCode"] for p in updates], [0, 0])
            self.assertTrue(all(p["outcome"] == "passed" for p in updates))
        finally:
            for process in processes:
                if process.poll() is None:
                    runner.common.reap(process, 1, group=True)

    def test_listener_nonzero_exit_is_fatal_and_preserved(self):
        self.listener_status = 17
        self.cleanup_code = 9
        status, result, popen, reap = self.run_mock()
        self.assertEqual(status, 17)
        phases = self.selected(result)[0]["phases"]
        self.assertEqual(phases[2]["listenerExitCode"], 17)
        self.assertEqual(phases[2]["outcome"], "failed")
        self.assertFalse(phases[4]["executed"])
        popen.assert_called_once()
        reap.assert_called_once()

    def test_listener_completion_timeout_is_fatal_and_reaped(self):
        self.listener_timeout = True
        status, result, popen, reap = self.run_mock()
        self.assertEqual(status, 124)
        phases = self.selected(result)[0]["phases"]
        self.assertEqual(phases[2]["listenerOutcome"], "timed_out")
        self.assertFalse(phases[4]["executed"])
        popen.return_value.wait.assert_called_once_with(timeout=120)
        reap.assert_called_once()

    def test_bad_yaml_is_an_explicit_preflight_error(self):
        config = self.fixture.root / "bad.yaml"
        config.write_text("[unterminated")
        with self.assertRaisesRegex(ValueError, "Invalid legacy configuration"):
            runner.load_yaml(config)


@unittest.skipUnless(os.name == "posix", "requires POSIX process groups")
class LegacyProcessTests(unittest.TestCase):
    def test_cleanup_probe_checks_exact_owned_names_and_propagates_errors(self):
        class LookupError(Exception):
            def __init__(self, code):
                self.code = code

            def get_error_code(self):
                return self.code

        connection = Mock()
        connection.lookupByName.side_effect = LookupError(1)
        connection.storagePoolLookupByName.side_effect = LookupError(2)
        connection.networkLookupByName.side_effect = LookupError(3)
        libvirt = SimpleNamespace(
            open=Mock(return_value=connection),
            libvirtError=LookupError,
            VIR_ERR_NO_DOMAIN=1,
            VIR_ERR_NO_STORAGE_POOL=2,
            VIR_ERR_NO_NETWORK=3,
        )
        prefix = "legacy-fixture-"
        with (
            patch.dict(sys.modules, {"libvirt": libvirt}),
            patch.object(sys, "argv", ["cleanup-check", prefix]),
        ):
            exec(runner.CLEANUP_CHECK, {})
            connection.lookupByName.assert_called_once_with(prefix + "-vm-0")
            connection.storagePoolLookupByName.assert_called_once_with(prefix + "-pool")
            connection.networkLookupByName.assert_called_once_with(prefix + "-network")
            connection.lookupByName.side_effect = None
            with self.assertRaisesRegex(RuntimeError, "Cleanup left owned resource"):
                exec(runner.CLEANUP_CHECK, {})
            connection.lookupByName.side_effect = LookupError(99)
            with self.assertRaises(LookupError):
                exec(runner.CLEANUP_CHECK, {})

    def test_real_config_preparation_preserves_existing_pre_post_hooks(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            source = RUNNER.parents[2] / "tests/e2e_tests"
            original = source / "trident_configurations/base/trident-config.yaml"
            config = root / "trident-config.yaml"
            config.write_bytes(original.read_bytes())
            hooks = runner.load_yaml(config)["scripts"]
            subprocess.run(
                [
                    sys.executable,
                    str(source / "helpers/edit_host_config.py"),
                    "--keypath",
                    str(root / "key"),
                    "--hostconfig",
                    str(config),
                    "--runtimeEnv",
                    "host",
                ],
                check=True,
                capture_output=True,
                timeout=30,
            )
            subprocess.run(
                [
                    sys.executable,
                    str(source / "helpers/inject_uefi_fallback_validation.py"),
                    "--hostconfig",
                    str(config),
                    "--runtimeEnv",
                    "host",
                    "--uefiFallbackMode",
                    "conservative",
                ],
                check=True,
                capture_output=True,
                timeout=30,
            )
            prepared = runner.load_yaml(config)
            self.assertEqual(prepared["scripts"], hooks)
            self.assertTrue(prepared["os"]["users"][0]["sshPublicKeys"])
            self.assertEqual(
                [h["runOn"] for h in prepared["health"]["checks"]],
                [["ab-update"], ["clean-install"]],
            )

    def test_group_reap_after_leader_exits_during_cancellation(self):
        with tempfile.TemporaryDirectory() as temp:
            with (
                patch.object(runner.common.subprocess, "Popen") as popen,
                patch.object(runner.common.os, "killpg") as kill,
            ):
                process = popen.return_value
                process.pid = 12345
                process.wait.side_effect = [subprocess.TimeoutExpired("fixture", 1), 0]
                status = runner.common.command(
                    ["fixture"],
                    Path(temp),
                    Path(temp) / "log",
                    1,
                    2,
                    group_cleanup=True,
                )
                self.assertEqual(status, 124)
                self.assertEqual(
                    kill.call_args_list,
                    [
                        unittest.mock.call(12345, signal.SIGTERM),
                        unittest.mock.call(12345, signal.SIGKILL),
                    ],
                )
                process.terminate.assert_not_called()

    def test_actual_pytest_junit_distinguishes_executed_pass_fail_skip(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / "test_fixture.py").write_text(
                "import pytest\n"
                "def test_pass(): assert True\n"
                "def test_fail(): assert False\n"
                "@pytest.mark.skip(reason='fixture')\n"
                "def test_skip(): assert False\n"
            )
            result = subprocess.run(
                [sys.executable, "-m", "pytest", "-q", "--junit-xml=report.xml"],
                cwd=root,
                capture_output=True,
                timeout=30,
            )
            self.assertEqual(result.returncode, 1, result.stdout)
            self.assertEqual(
                [
                    case["outcome"]
                    for case in runner.pytest_results(root / "report.xml")
                ],
                ["passed", "failed", "skipped"],
            )

    def test_real_sleeping_command_timeout_is_bounded_and_reaped(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            status = runner.common.command(
                [sys.executable, "-c", "import time; time.sleep(60)"],
                root,
                root / "log",
                0.1,
                0.2,
                group_cleanup=True,
            )
            self.assertEqual(status, 124)


if __name__ == "__main__":
    unittest.main()
