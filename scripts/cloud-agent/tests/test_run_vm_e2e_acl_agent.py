import argparse
import importlib.util
import json
import os
import signal
import subprocess
import sys
import tempfile
import unittest

from contextlib import contextmanager
from pathlib import Path
from unittest.mock import patch

RUNNER = Path(__file__).resolve().parents[1] / "run-vm-e2e-acl-agent.py"
SPEC = importlib.util.spec_from_file_location("acl_runner", RUNNER)
runner = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(runner)
REVISION = "a" * 40
IMAGE = "docker.io/library/postgres@sha256:" + "b" * 64
CONTAINER = "c" * 64


class AclAgentRunnerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.bundle = self.root / "bundle with spaces"
        self.bundle.mkdir()
        files = {}
        for name, artifact in (
            ("bin/storm-trident", "go-tools"),
            (runner.BASE, "image-acl-agent-base"),
            (runner.UPDATE, "image-acl-agent-update"),
        ):
            path = self.bundle / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(b"owned synthetic fixture, not an image or evidence")
            files[name] = {
                "artifact": artifact,
                "size": path.stat().st_size,
                "sha256": runner.common.digest(path),
            }
        (self.bundle / "bin/storm-trident").chmod(0o755)
        self.manifest = {
            "schemaVersion": 1,
            "candidateRevision": REVISION,
            "producer": {
                "organization": "mariner-org",
                "projectId": runner.common.PROJECT_ID,
                "definitionId": 5073,
                "buildId": 123,
                "sourceVersion": REVISION,
            },
            "files": files,
            "aclAgent": {
                "configRevision": REVISION,
                "postgresImage": IMAGE,
                "targetVersion": "202507.28.0",
                "hostEndpointIP": "192.168.122.1",
            },
        }
        self.key = self.root / "existing-key-fixture"
        self.key.write_text("placeholder, not a private key")
        self.args = argparse.Namespace(
            bundle=self.bundle,
            manifest=self.bundle / "provenance.json",
            manifest_sha256="",
            expected_revision=REVISION,
            output=self.root / "output with spaces",
            ssh_private_key_path=self.key,
            timeout_seconds=120,
            cleanup_grace_seconds=5,
        )
        self.save_manifest()
        self.commands = []
        self.locked = False
        self.status = 0
        self.report = self.junit()
        self.domain = False
        self.owner = ""
        self.foreign_disk = False
        self.container = False
        self.image_platform = "linux/amd64"
        self.docker_commands = []
        self.cleanup_interrupt = False
        self.cleanup_failure = False

    def save_manifest(self):
        self.args.manifest.write_text(json.dumps(self.manifest))
        self.args.manifest_sha256 = runner.common.digest(self.args.manifest)

    def junit(self, overrides=None):
        overrides = overrides or {}
        return (
            "<testsuite>"
            + "".join(
                f'<testcase name="{name}" status="PASS">'
                + overrides.get(name, "")
                + "</testcase>"
                for name in runner.CASES
            )
            + "</testsuite>"
        )

    @contextmanager
    def lock(self):
        self.assertFalse(self.locked)
        self.locked = True
        try:
            yield
        finally:
            self.locked = False

    def mock_command(self, argv, cwd, log, timeout, grace):
        self.assertTrue(self.locked)
        self.commands.append(argv)
        log.write_text("")
        if argv[0] != "virsh":
            self.assertEqual(timeout, 120)
            self.assertEqual(grace, 5)
            self.assertTrue((cwd / "bin/docker").is_file())
            self.owner = argv[argv.index("--name") + 1]
            self.domain = True
            self.container = True
            if self.report is not None:
                Path(argv[argv.index("-j") + 1]).write_text(self.report)
            return self.status
        if log.name.startswith("cleanup") and self.cleanup_interrupt:
            self.assertEqual(signal.getsignal(signal.SIGINT), signal.SIG_IGN)
            self.assertEqual(signal.getsignal(signal.SIGTERM), signal.SIG_IGN)
            os.kill(os.getpid(), signal.SIGTERM)
            os.kill(os.getpid(), signal.SIGINT)
        operation = argv[3]
        if operation == "list":
            log.write_text(self.owner if self.domain else "")
        elif operation == "dumpxml":
            disk = self.args.output / "artifacts/booted.qcow2"
            if self.foreign_disk:
                disk = self.root / "foreign.qcow2"
            log.write_text(
                f'<domain><devices><disk><source file="{disk}"/></disk></devices></domain>'
            )
        elif operation == "domstate":
            log.write_text("running")
        elif operation == "undefine":
            if self.cleanup_failure:
                return 29
            self.domain = False
        return 0

    def mock_docker(self, docker, argv, timeout=30):
        self.assertTrue(self.locked)
        self.docker_commands.append(argv)
        if argv[0] == "image":
            return self.image_platform
        if argv[0] == "ps":
            return CONTAINER if self.container else ""
        if argv[0] == "inspect":
            return self.owner
        if argv[0] == "rm":
            self.container = False
        return ""

    def run_mock(self, host_error=None, docker_error=None):
        with (
            patch.object(runner.common, "host_lock", side_effect=self.lock),
            patch.object(runner.common, "check_host", side_effect=host_error),
            patch.object(runner.common, "command", side_effect=self.mock_command),
            patch.object(runner, "check_network"),
            patch.object(
                runner, "docker_call", side_effect=docker_error or self.mock_docker
            ),
            patch.object(
                runner.shutil, "which", side_effect=lambda name: "/usr/bin/" + name
            ),
            patch.object(runner.os, "geteuid", return_value=0),
        ):
            status = runner.execute(self.args)
        return status, json.loads((self.args.output / "results.json").read_text())

    def test_existing_single_suite_command_and_owned_cleanup_under_lock(self):
        previous = os.environ.get("PATH")
        status, result = self.run_mock()
        self.assertEqual(status, 0)
        self.assertEqual(result["outcome"], "passed")
        self.assertEqual(result["provenance"], self.manifest)
        self.assertEqual(os.environ.get("PATH"), previous)
        self.assertFalse(self.locked or self.domain or self.container)
        self.assertEqual(
            result["counts"],
            {
                "selected": 8,
                "executed": 8,
                "passed": 8,
                "failed": 0,
                "skipped": 0,
                "unavailable": 0,
                "not_run": 0,
            },
        )
        storm = [cmd for cmd in self.commands if cmd[0] != "virsh"]
        self.assertEqual(len(storm), 1)
        argv = storm[0]
        self.assertEqual(
            argv[:3], [str(self.bundle / "bin/storm-trident"), "run", "aclagent"]
        )
        self.assertNotIn("-a", argv)
        self.assertNotIn("--verbose", argv)
        for flag, value in (
            ("--test-case-to-run", "all"),
            ("--platform", "qemu"),
            ("--postgres-image", IMAGE),
            ("--target-version", "202507.28.0"),
            ("--node-name", "trident-acl-agent-testimg"),
            ("--name", result["owner"]),
            ("--ssh-private-key-path", str(self.key)),
            ("--host-endpoint-ip", "192.168.122.1"),
            ("--api-server-port", "18080"),
            ("--nebraska-port", "18081"),
            ("--image-server-port", "18082"),
            ("--image-pattern", r"^trident-vm-acl-agent-testimage\.qcow2$"),
            ("--cosi-pattern", r"^trident-vm-acl-agent-update-testimage\.cosi$"),
        ):
            self.assertEqual(argv[argv.index(flag) + 1], value)
        self.assertIn("--force-cleanup", argv)
        for path in (runner.BASE, runner.UPDATE):
            staged = self.args.output / "artifacts" / Path(path).name
            self.assertEqual(staged.stat().st_ino, (self.bundle / path).stat().st_ino)
        runner.verify(self.args)
        self.assertFalse((self.args.output / self.key.name).exists())
        self.assertTrue(any("destroy" in cmd for cmd in self.commands))
        self.assertTrue(
            any("undefine" in cmd and "--nvram" in cmd for cmd in self.commands)
        )
        self.assertIn(["rm", "-f", CONTAINER], self.docker_commands)

    def test_pins_roles_config_and_registry_rejected_before_host_or_commands(self):
        original = json.dumps(self.manifest)
        mutations = (
            lambda: self.manifest.update(candidateRevision="d" * 40),
            lambda: self.manifest["files"][runner.BASE].update(sha256="0" * 64),
            lambda: self.manifest["files"][runner.BASE].update(artifact="baseline"),
            lambda: self.manifest["files"][runner.UPDATE].update(artifact="baseline"),
            lambda: self.manifest["aclAgent"].update(configRevision="e" * 40),
            lambda: self.manifest["aclAgent"].update(
                postgresImage="postgres:16-alpine"
            ),
            lambda: self.manifest["aclAgent"].update(postgresImage="host/repo:latest"),
            lambda: self.manifest["aclAgent"].update(targetVersion="latest"),
            lambda: self.manifest["aclAgent"].update(hostEndpointIP="0.0.0.0"),
            lambda: self.manifest.pop("aclAgent"),
        )
        for index, mutate in enumerate(mutations):
            with self.subTest(index=index):
                self.manifest = json.loads(original)
                mutate()
                self.save_manifest()
                self.args.output = self.root / f"reject-{index}"
                status, result = self.run_mock()
                self.assertEqual(status, 1)
                self.assertEqual(result["outcome"], "unavailable")
                self.assertFalse(result["invoked"])
                self.assertEqual(result["counts"]["not_run"], 8)
                self.assertEqual(self.commands, [])
                self.assertEqual(self.docker_commands, [])

    def test_bad_manifest_pin_and_missing_key_fail_closed(self):
        self.args.manifest_sha256 = "0" * 64
        status, _ = self.run_mock()
        self.assertEqual(status, 1)
        self.assertEqual(self.commands, [])
        self.save_manifest()
        self.args.output = self.root / "missing-key-output"
        self.args.ssh_private_key_path = self.root / "absent-key"
        status, result = self.run_mock()
        self.assertEqual(status, 1)
        self.assertEqual(result["outcome"], "unavailable")
        self.assertEqual(self.commands, [])

    def test_key_must_remain_external_and_is_never_opened(self):
        self.args.ssh_private_key_path = self.bundle / "external-key-not-allowed"
        with patch.object(runner.common, "verify_bundle") as verify:
            with self.assertRaisesRegex(ValueError, "external"):
                runner.verify(self.args)
            verify.assert_not_called()
        self.args.ssh_private_key_path = self.key
        original = Path.open

        def guarded_open(path, *args, **kwargs):
            self.assertNotEqual(path, self.key, "Runner must never open the SSH key")
            return original(path, *args, **kwargs)

        with patch.object(Path, "open", guarded_open):
            status, _ = self.run_mock()
        self.assertEqual(status, 0)

    def test_missing_host_or_registry_prerequisites_are_unavailable(self):
        for index, error in enumerate(
            (ValueError("KVM unavailable"), OSError("network unavailable"))
        ):
            with self.subTest(error=type(error)):
                self.args.output = self.root / f"unavailable-{index}"
                status, result = self.run_mock(host_error=error)
                self.assertEqual(status, 1)
                self.assertEqual(result["outcome"], "unavailable")
                self.assertFalse(result["invoked"])
        self.args.output = self.root / "missing-image"
        status, result = self.run_mock(docker_error=ValueError("image unavailable"))
        self.assertEqual(status, 1)
        self.assertFalse(result["invoked"])
        self.assertEqual(result["outcome"], "unavailable")

    def test_cached_postgres_must_be_linux_amd64_before_any_vm_execution(self):
        self.image_platform = "linux/arm64"
        status, result = self.run_mock()
        self.assertEqual(status, 1)
        self.assertEqual(result["outcome"], "unavailable")
        self.assertFalse(result["invoked"])
        self.assertIn("linux/amd64", result["error"])
        self.assertEqual(self.commands, [])

    def test_first_failure_pending_and_skip_accounting(self):
        self.status = 23
        self.report = self.junit(
            {
                "run-ab-update": "<failure/>",
                "run-rollback": '<skipped type="NotRun"/>',
                "collect-logs": '<skipped type="Skipped"/>',
                "cleanup-vm": '<skipped type="Pending"/>',
            }
        )
        status, result = self.run_mock()
        self.assertEqual(status, 23)
        self.assertEqual(result["firstFailedCase"], "run-ab-update")
        self.assertEqual(result["counts"]["passed"], 4)
        self.assertEqual(result["counts"]["failed"], 1)
        self.assertEqual(result["counts"]["not_run"], 2)
        self.assertEqual(result["counts"]["skipped"], 1)
        self.assertEqual(result["counts"]["executed"], 5)
        self.assertFalse(self.domain or self.container)

    def test_missing_malformed_or_partial_report_never_claims_not_run_or_pass(self):
        for index, report in enumerate(
            (
                None,
                "not XML",
                '<testsuite><testcase name="deploy-vm" status="PASS"/></testsuite>',
            )
        ):
            with self.subTest(index=index):
                self.args.output = self.root / f"report-{index}"
                self.report = report
                self.status = 19
                status, result = self.run_mock()
                self.assertEqual(status, 19)
                self.assertEqual(result["outcome"], "failed")
                self.assertEqual(result["counts"]["not_run"], 0)
                self.assertGreaterEqual(result["counts"]["unavailable"], 7)
                self.assertFalse(self.domain or self.container)

    def test_zero_exit_with_skip_failure_pending_or_missing_report_is_failure(self):
        for index, report in enumerate(
            (
                None,
                self.junit({"run-ab-update": "<failure/>"}),
                self.junit({"run-ab-update": '<skipped type="Skipped"/>'}),
                self.junit({"run-ab-update": '<skipped type="NotRun"/>'}),
            )
        ):
            with self.subTest(index=index):
                self.args.output = self.root / f"false-success-{index}"
                self.report = report
                status, result = self.run_mock()
                self.assertEqual(status, 1)
                self.assertEqual(result["outcome"], "failed")
                self.assertFalse(self.domain or self.container)

    def test_timeout_and_cancellation_keep_original_status_and_mask_cleanup_signals(
        self,
    ):
        for status in (124, 130):
            with self.subTest(status=status):
                self.args.output = self.root / f"interrupt-{status}"
                self.status = status
                self.report = None
                self.cleanup_interrupt = True
                self.cleanup_failure = True
                actual, result = self.run_mock()
                self.assertEqual(actual, status)
                self.assertTrue(result["cleanupErrors"])
                self.assertFalse(self.container)
                self.assertEqual(result["counts"]["unavailable"], 8)

    def test_real_process_timeout_finishes_before_locked_fallback_cleanup(self):
        fixture_command = self.mock_command
        command = runner.common.command
        stopped = self.root / "stopped"
        script = (
            "import signal, sys, time\n"
            "from pathlib import Path\n"
            "def stop(*args):\n"
            "    Path(sys.argv[1]).write_text('stopped')\n"
            "    sys.exit(0)\n"
            "signal.signal(signal.SIGTERM, stop)\n"
            "while True: time.sleep(0.01)\n"
        )

        def real_storm(argv, cwd, log, timeout, grace):
            fixture_status = fixture_command(argv, cwd, log, timeout, grace)
            if argv[0] == "virsh":
                if log.name.startswith("cleanup"):
                    self.assertTrue(stopped.is_file())
                return fixture_status
            return command(
                [sys.executable, "-c", script, str(stopped)], cwd, log, 0.3, 1
            )

        self.mock_command = real_storm
        status, result = self.run_mock()
        self.assertEqual(status, 124)
        self.assertEqual(result["processExitCode"], 124)
        self.assertEqual(result["outcome"], "failed")
        self.assertEqual(result["cleanupErrors"], [])
        self.assertFalse(self.locked or self.domain or self.container)

    def test_cleanup_failure_fails_success_but_never_replaces_primary_failure(self):
        self.cleanup_failure = True
        for expected in (1, 17):
            self.status = 0 if expected == 1 else expected
            self.args.output = self.root / f"cleanup-{expected}"
            status, result = self.run_mock()
            self.assertEqual(status, expected)
            self.assertEqual(result["outcome"], "failed")
            self.assertTrue(result["cleanupErrors"])
            self.assertFalse(self.container)

    def test_unowned_disk_is_not_destroyed_but_container_cleanup_still_runs(self):
        self.foreign_disk = True
        status, result = self.run_mock()
        self.assertEqual(status, 1)
        self.assertTrue(result["cleanupErrors"])
        self.assertFalse(
            any("destroy" in cmd or "undefine" in cmd for cmd in self.commands)
        )
        self.assertFalse(self.container)

    def interrupted_start(self, argv, cwd, log, timeout, grace):
        status = AclAgentRunnerTests.mock_command(self, argv, cwd, log, timeout, grace)
        if argv[0] != "virsh":
            self.container = False
            config = json.loads((cwd / "docker-proxy.json").read_text())
            error = subprocess.TimeoutExpired("owned synthetic Docker request", 30)
            with patch.object(runner, "docker_call", side_effect=error):
                with self.assertRaises(subprocess.TimeoutExpired):
                    runner.docker_proxy(config, ["run", "-d", "--rm", IMAGE])
        return status

    def test_delayed_daemon_registration_is_reconciled_without_clearing_uncertainty(
        self,
    ):
        self.mock_command = self.interrupted_start
        fixture_docker = self.mock_docker
        for index, primary in enumerate((0, 23, 124, 130)):
            with self.subTest(primary=primary):
                self.args.output = self.root / f"delayed-{index}"
                self.status = primary
                scans = []
                clock = [0]

                def delayed_docker(docker, argv, timeout=30):
                    if argv[0] != "image":
                        self.assertGreater(timeout, 0)
                        self.assertLessEqual(
                            timeout, self.args.cleanup_grace_seconds - clock[0]
                        )
                    if argv[0] == "ps":
                        scans.append(argv)
                        if len(scans) == 3:
                            self.container = True
                    return fixture_docker(docker, argv, timeout)

                def sleep(seconds):
                    self.assertTrue(self.locked)
                    clock[0] += seconds

                self.mock_docker = delayed_docker
                with (
                    patch.object(
                        runner.time, "monotonic", side_effect=lambda: clock[0]
                    ),
                    patch.object(runner.time, "sleep", side_effect=sleep),
                ):
                    status, result = self.run_mock()
                self.assertEqual(status, primary or 1)
                self.assertEqual(result["processExitCode"], primary)
                self.assertEqual(result["outcome"], "failed")
                self.assertTrue(
                    any(
                        "Unresolved Postgres creation" in error
                        for error in result["cleanupErrors"]
                    )
                )
                self.assertIn(["rm", "-f", CONTAINER], self.docker_commands)
                self.assertGreater(len(scans), 2)
                self.assertEqual(clock[0], self.args.cleanup_grace_seconds)
                self.assertFalse(self.locked or self.domain or self.container)

    def test_registration_after_reconciliation_bound_requires_outer_host_disposal(self):
        self.mock_command = self.interrupted_start
        self.status = 124
        clock = [0]

        def sleep(seconds):
            self.assertTrue(self.locked)
            clock[0] += seconds

        with (
            patch.object(runner.time, "monotonic", side_effect=lambda: clock[0]),
            patch.object(runner.time, "sleep", side_effect=sleep),
        ):
            status, result = self.run_mock()
        self.assertEqual(status, 124)
        self.assertFalse(self.locked)
        self.assertFalse(self.container)
        # The fake daemon accepts the earlier request only after reconciliation.
        self.container = True
        self.assertTrue(
            any(
                "outer-host disposal required" in error
                for error in result["cleanupErrors"]
            )
        )
        self.assertEqual(result["outcome"], "failed")
        self.assertEqual(clock[0], self.args.cleanup_grace_seconds)

    def test_completed_creation_receipt_allows_verified_owned_cleanup(self):
        fixture_command = self.mock_command

        def accepted_start(argv, cwd, log, timeout, grace):
            status = fixture_command(argv, cwd, log, timeout, grace)
            if argv[0] != "virsh":
                config = json.loads((cwd / "docker-proxy.json").read_text())

                def created(docker, command, timeout=30):
                    if command[0] == "run":
                        return CONTAINER
                    return "healthy" if "Health" in command[2] else self.owner

                with patch.object(runner, "docker_call", side_effect=created):
                    self.assertEqual(
                        runner.docker_proxy(config, ["run", "-d", "--rm", IMAGE]),
                        CONTAINER,
                    )
            return status

        self.mock_command = accepted_start
        status, result = self.run_mock()
        self.assertEqual(status, 0)
        self.assertEqual(result["cleanupErrors"], [])
        self.assertFalse(self.locked or self.container or self.domain)


class DockerProxyTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.config = {
            "docker": "/owned/docker",
            "owner": "trident-acl-fixture",
            "image": IMAGE,
            "starts": self.temp.name,
        }
        self.calls = []
        self.health = "healthy"
        self.owner = self.config["owner"]

    def docker(self, docker, argv, timeout=30):
        self.calls.append(argv)
        if argv[0] == "run":
            return CONTAINER
        if argv[0] == "inspect":
            return self.health if "Health" in argv[2] else self.owner
        return "127.0.0.1:32123"

    def test_pinned_labeled_pull_never_health_checked_canonical_service(self):
        with patch.object(runner, "docker_call", side_effect=self.docker):
            result = runner.docker_proxy(
                self.config, ["run", "-d", "--rm", "-p", "127.0.0.1::5432", IMAGE]
            )
            self.assertEqual(result, CONTAINER)
            command = self.calls[0]
            self.assertIn("--pull=never", command)
            self.assertIn(f"{runner.LABEL}={self.owner}", command)
            self.assertIn("--health-cmd", command)
            self.assertTrue(
                command[command.index("--name") + 1].startswith(self.owner + "-")
            )
            self.assertEqual(command[-1], IMAGE)
            self.assertEqual(
                runner.docker_proxy(self.config, ["port", CONTAINER, "5432/tcp"]),
                "127.0.0.1:32123",
            )
            runner.docker_proxy(self.config, ["rm", "-f", CONTAINER])

    def test_creation_intent_is_fsynced_before_request_and_receipt_after_completion(
        self,
    ):
        fixture_docker = self.docker
        with patch.object(runner.os, "fsync", wraps=os.fsync) as fsync:

            def creation(docker, argv, timeout=30):
                if argv[0] == "run":
                    name = argv[argv.index("--name") + 1]
                    intent = Path(self.config["starts"]) / f"{name}.intent"
                    self.assertEqual(intent.read_text().strip(), name)
                    self.assertFalse(intent.with_suffix(".created").exists())
                    self.assertEqual(fsync.call_count, 2)
                return fixture_docker(docker, argv, timeout)

            with patch.object(runner, "docker_call", side_effect=creation):
                runner.docker_proxy(self.config, ["run", "-d", IMAGE])
            self.assertEqual(fsync.call_count, 4)
        intents = list(Path(self.config["starts"]).glob("*.intent"))
        self.assertEqual(len(intents), 1)
        self.assertEqual(
            intents[0].with_suffix(".created").read_text().strip(), CONTAINER
        )

    def test_interrupted_or_timed_out_creation_remains_unresolved(self):
        for error in (KeyboardInterrupt(), subprocess.TimeoutExpired("fixture", 30)):
            with self.subTest(error=type(error)):
                with patch.object(runner, "docker_call", side_effect=error):
                    with self.assertRaises(type(error)):
                        runner.docker_proxy(self.config, ["run", "-d", IMAGE])
                self.assertFalse(list(Path(self.config["starts"]).glob("*.created")))
        self.assertEqual(len(list(Path(self.config["starts"]).glob("*.intent"))), 2)

    def test_failed_intent_persistence_prevents_creation_request(self):
        with (
            patch.object(
                runner.os, "fsync", side_effect=OSError("fixture disk failure")
            ),
            patch.object(runner, "docker_call") as docker,
        ):
            with self.assertRaises(OSError):
                runner.docker_proxy(self.config, ["run", "-d", IMAGE])
            docker.assert_not_called()

    def test_unhealthy_and_bounded_startup_never_return_container_success(self):
        with patch.object(runner, "docker_call", side_effect=self.docker):
            self.health = "unhealthy"
            with self.assertRaisesRegex(ValueError, "health check"):
                runner.docker_proxy(self.config, ["run", "-d", IMAGE])
            self.health = "starting"
            with (
                patch.object(runner.time, "monotonic", side_effect=[0, 0, 41]),
                patch.object(runner.time, "sleep"),
                self.assertRaisesRegex(ValueError, "timed out"),
            ):
                runner.docker_proxy(self.config, ["run", "-d", IMAGE])

    def test_foreign_containers_mutable_images_and_global_operations_rejected(self):
        with patch.object(runner, "docker_call", side_effect=self.docker):
            for argv in (
                ["system", "prune"],
                ["rm", "-f", "foreign"],
                ["run", "postgres:latest"],
                ["run", "--label=x=y", IMAGE],
            ):
                with self.subTest(argv=argv), self.assertRaises(ValueError):
                    runner.docker_proxy(self.config, argv)
            self.assertEqual(self.calls, [])
            self.owner = "foreign"
            with self.assertRaisesRegex(ValueError, "unowned"):
                runner.docker_proxy(self.config, ["rm", "-f", CONTAINER])
            self.assertFalse(any(cmd[0] == "rm" for cmd in self.calls))

    def test_real_suite_local_wrapper_uses_fake_docker_without_logging_environment(
        self,
    ):
        with tempfile.TemporaryDirectory(prefix="acl proxy fixture ") as directory:
            root = Path(directory)
            docker = root / "fake-docker"
            docker.write_text(
                "#!/usr/bin/env python3\nimport sys\n"
                f"print({CONTAINER!r} if sys.argv[1] == 'run' else "
                f"('healthy' if 'Health' in sys.argv[3] else {self.owner!r}))\n"
            )
            docker.chmod(0o700)
            config = dict(self.config, docker=str(docker))
            previous = os.environ.get("PATH")
            with runner.service_environment(root, config):
                self.assertEqual(os.environ["TF_BUILD"], "false")
                self.assertEqual(os.environ["LC_ALL"], "C")
                process = subprocess.run(
                    ["docker", "run", "-d", "--rm", IMAGE],
                    capture_output=True,
                    text=True,
                    timeout=10,
                )
            self.assertEqual(process.returncode, 0, process.stderr)
            self.assertEqual(process.stdout.strip(), CONTAINER)
            self.assertEqual(process.stderr, "")
            self.assertEqual(os.environ.get("PATH"), previous)

    def test_existing_network_and_endpoint_ports_must_be_available(self):
        config = {"hostEndpointIP": "192.168.122.1"}
        with tempfile.TemporaryDirectory() as directory:
            with (
                patch.object(
                    runner,
                    "operation",
                    return_value='<network><ip address="192.168.122.1"/></network>',
                ),
                patch.object(runner.socket, "socket") as socket,
            ):
                runner.check_network(Path(directory), config, None)
                self.assertEqual(
                    [
                        call.args[0]
                        for call in socket.return_value.__enter__.return_value.bind.call_args_list
                    ],
                    [("192.168.122.1", port) for port in (18080, 18081, 18082)],
                )
                socket.return_value.__enter__.return_value.bind.side_effect = OSError(
                    "address unavailable"
                )
                with self.assertRaises(OSError):
                    runner.check_network(Path(directory), config, None)
            with patch.object(
                runner,
                "operation",
                return_value='<network><ip address="192.168.123.1"/></network>',
            ):
                with self.assertRaisesRegex(ValueError, "default libvirt network"):
                    runner.check_network(Path(directory), config, None)

    def test_docker_errors_do_not_reveal_command_or_environment(self):
        with patch.object(
            runner.subprocess,
            "run",
            return_value=subprocess.CompletedProcess(
                [], 1, b"", b"sensitive fixture diagnostics"
            ),
        ):
            with self.assertRaisesRegex(
                ValueError, "^Docker prerequisite/service operation failed$"
            ):
                runner.docker_call("/fake/docker", ["run", "TEST_FIXTURE=owned"])


if __name__ == "__main__":
    unittest.main()
