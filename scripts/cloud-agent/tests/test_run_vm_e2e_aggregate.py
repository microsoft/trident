import argparse
import importlib.util
import io
import json
import os
import pwd
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import threading
import time
import unittest

from contextlib import redirect_stderr
from pathlib import Path
from unittest.mock import patch

import test_run_vm_e2e_install as fixtures
import test_run_vm_e2e_acl_agent as acl_fixtures

RUNNER = Path(__file__).resolve().parents[1] / "run-vm-e2e-aggregate.py"
SPEC = importlib.util.spec_from_file_location("vm_aggregate", RUNNER)
runner = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(runner)


class AggregateTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.args = argparse.Namespace(
            source=self.root / "source",
            functional_harness=self.root / "adapter",
            functional_harness_revision=runner.FUNCTIONAL_REVISION,
            expected_tool_revision=runner.BASE,
            tool_sha256="b" * 64,
            ssh_private_key=self.root / "external-key",
            output=self.root / "output",
            timeout_seconds=2,
            cleanup_grace_seconds=1,
            lane_timeout_seconds=3,
            shutdown_grace_seconds=1,
            expected_hostname="owned-test-host",
            qemu_user="nobody",
        )
        self.args.source.mkdir()
        self.args.functional_harness.mkdir()
        adapter = self.args.functional_harness / runner.FUNCTIONAL
        adapter.parent.mkdir(parents=True)
        adapter.write_text("fixture supports --expected-source-revision\n")
        for family in runner.FAMILIES:
            fixture = fixtures.InstallRunnerTests()
            fixture.setUp()
            self.addCleanup(fixture.doCleanups)
            fixture.manifest["candidateRevision"] = runner.PRODUCT
            fixture.manifest["producer"]["sourceVersion"] = runner.PRODUCT
            fixture.manifest["producer"]["buildId"] = (
                1266405 if family in ("install", "legacy") else 1266514
            )
            fixture.save_manifest()
            prefix = family.replace("-", "_")
            setattr(self.args, prefix + "_bundle", fixture.bundle)
            setattr(self.args, prefix + "_manifest", fixture.manifest_path)
            setattr(
                self.args, prefix + "_manifest_sha256", fixture.args.manifest_sha256
            )
        self.args.ssh_private_key.write_text("non-cryptographic owned unit fixture\n")
        self.args.ssh_private_key.chmod(0o600)

    def rows(self):
        return runner.lanes(self.args, {"z_vm-host": {}, "a_vm-host": {}}, ["base"])

    def fake_commands(self, rows, failure=None, source=None):
        events = self.root / "events"
        for row in rows:
            code = source or (
                "from pathlib import Path; "
                f"p=Path({str(events)!r}); "
                f"p.open('a').write({row['lane']!r}+'\\n'); "
                f"raise SystemExit({29 if row['lane'] == failure else 0})"
            )
            row["argv"] = [sys.executable, "-c", code]
        return events

    def verify(self):
        def driver_bytes(root, *args):
            return (runner.ROOT / args[-1].split(":", 1)[1]).read_bytes()

        with patch.object(runner, "checkout"), patch.object(
            runner, "git", side_effect=driver_bytes
        ), patch.object(runner.lifecycle, "verify_lifecycle"), patch.object(
            runner.acl, "verify"
        ):
            return runner.verify_inputs(self.args)

    def test_fixed_entrypoints_and_fresh_sequential_process_order(self):
        rows = self.rows()
        functional = rows[0]["argv"]
        self.assertIn(str(self.args.functional_harness / runner.FUNCTIONAL), functional)
        flag = functional.index("--expected-source-revision")
        self.assertEqual(functional[flag + 1], runner.PRODUCT)
        for row in rows[1:]:
            self.assertEqual(
                Path(row["argv"][1]).name, f"run-vm-e2e-{row['family']}.py"
            )
        events = self.fake_commands(rows)
        observed = []
        with patch.object(runner, "evidence", return_value=(False, {})):
            code = runner.run_lanes(
                rows,
                self.args,
                {"signal": None},
                lambda: observed.append(
                    events.read_text().splitlines() if events.exists() else []
                ),
            )
        self.assertEqual(code, 0)
        self.assertEqual(
            events.read_text().splitlines(),
            [
                "functional-smoke-full",
                "install-a_vm-host",
                "install-z_vm-host",
                "legacy-vm-host",
                "qemu-servicing-rollback",
                "acl-agent",
            ],
        )
        self.assertEqual(
            observed,
            [
                [r["lane"] for r in rows[:count]]
                for index in range(len(rows))
                for count in (index, index + 1)
            ],
        )
        self.assertTrue(all(r["outcome"] == "passed" for r in rows))

    def test_preflight_lock_is_released_before_real_child_driver_boundary(self):
        lock = self.root / "host.lock"
        lock.touch()
        work = self.root / "preflight"
        work.mkdir()

        def locked(*args):
            with self.assertRaises(BlockingIOError), runner.install.host_lock():
                self.fail("Preflight lost its lock")

        with patch.object(runner.install, "HOST_LOCK", lock), patch.dict(
            os.environ,
            EXCLUSIVE_DISPOSABLE_1ES="true",
            GITHUB_ACTIONS="true",
            GITHUB_RUN_ID="1",
            GITHUB_RUN_ATTEMPT="1",
        ), patch.object(
            runner.socket, "gethostname", return_value=self.args.expected_hostname
        ), patch.object(
            runner.lifecycle, "preflight", side_effect=locked
        ), patch.object(
            runner.install, "command", return_value=0
        ), patch.object(
            runner.shutil, "which", return_value="/unit-fixture-tool"
        ), patch.object(
            runner.acl, "docker_call", return_value="linux/amd64"
        ), patch.object(
            runner.acl, "check_network"
        ), patch.object(
            runner.acl, "domains"
        ), patch.object(
            runner.socket, "socket"
        ), patch.object(
            runner, "discover", return_value=({}, [], [])
        ):
            runner.preflight(
                self.args,
                {
                    "lifecycle": {},
                    "acl-agent": {"aclAgent": {"postgresImage": "unit-fixture-image"}},
                },
                work,
            )
        rows = self.rows()[:1]
        self.fake_commands(
            rows,
            source=(
                "import fcntl\n"
                f"with open({str(lock)!r}, 'rb') as stream:\n"
                " fcntl.flock(stream, fcntl.LOCK_EX | fcntl.LOCK_NB)\n"
            ),
        )
        with patch.object(runner, "evidence", return_value=(False, {})):
            self.assertEqual(
                runner.run_lanes(rows, self.args, {"signal": None}, lambda: None), 0
            )

    @unittest.skipUnless(
        os.geteuid() == 0 and shutil.which("setpriv"),
        "requires root and setpriv for real ACL cross-UID validation",
    )
    def test_real_acl_child_can_open_owned_disk_without_exposing_private_evidence(self):
        fixture = acl_fixtures.AclAgentRunnerTests()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        fixture.manifest["candidateRevision"] = runner.PRODUCT
        fixture.manifest["producer"]["sourceVersion"] = runner.PRODUCT
        fixture.manifest["aclAgent"]["configRevision"] = runner.PRODUCT
        fixture.args.expected_revision = runner.PRODUCT
        fixture.args.output = self.args.output / "acl-agent"
        fixture.save_manifest()
        self.args.acl_agent_manifest_sha256 = fixture.args.manifest_sha256
        for name in (acl_fixtures.runner.BASE, acl_fixtures.runner.UPDATE):
            (fixture.bundle / name).chmod(0o600)
        fixture.key.chmod(0o600)
        identity = pwd.getpwnam(self.args.qemu_user)
        self.root.chmod(0o755)
        ancestor = self.root.stat()
        access = fixture.args.output / "qemu-access.json"
        probe = """
import json, sys
try:
    with open(sys.argv[1], "r+b") as disk:
        disk.write(b"owned synthetic disk")
        disk.seek(0)
        assert disk.read() == b"owned synthetic disk"
except PermissionError:
    print(json.dumps({"errorType": "PermissionError"}))
    sys.exit(1)
print(json.dumps({"diskReadWrite": True}))
"""
        child = """
import argparse, json, os, subprocess, sys
from pathlib import Path
import test_run_vm_e2e_acl_agent as fixtures
fixture = fixtures.AclAgentRunnerTests()
fixture.setUp()
values = json.loads(Path(sys.argv[1]).read_text())
for name in ("bundle", "manifest", "output", "ssh_private_key_path"):
    values[name] = Path(values[name])
fixture.args = argparse.Namespace(**values)
original = fixture.mock_command
def command(argv, cwd, log, timeout, grace):
    status = original(argv, cwd, log, timeout, grace)
    if argv[0] != "virsh":
        disk = cwd / "artifacts/booted.qcow2"
        disk.write_bytes(b"owned synthetic disk")
        os.chown(disk, int(sys.argv[2]), int(sys.argv[3]))
        result = subprocess.run(
            [sys.argv[4], "--reuid", sys.argv[2], "--regid", sys.argv[3],
             "--clear-groups", sys.executable, "-c", sys.argv[5], str(disk)],
            capture_output=True, text=True,
        )
        (cwd / "qemu-access.json").write_text(result.stdout)
        result.check_returncode()
    return status
fixture.mock_command = command
try:
    status, _ = fixture.run_mock()
finally:
    fixture.doCleanups()
sys.exit(status)
"""
        with runner.lifecycle.private_creation():
            self.args.output.mkdir(mode=0o700)
            runner.lifecycle.qemu_searchable(self.args.output, identity)
            summary = self.args.output / "summary.json"
            summary.write_text("{}\n")
            arguments = self.root / "acl-arguments.json"
            arguments.write_text(
                json.dumps(
                    {
                        key: str(value) if isinstance(value, Path) else value
                        for key, value in vars(fixture.args).items()
                    }
                )
            )
            rows = self.rows()[-1:]
            rows[0]["argv"] = [
                sys.executable,
                "-c",
                f"import sys; sys.path.insert(0, {str(RUNNER.parent / 'tests')!r})\n"
                + child,
                str(arguments),
                str(identity.pw_uid),
                str(identity.pw_gid),
                shutil.which("setpriv"),
                probe,
            ]
            code = runner.run_lanes(rows, self.args, {"signal": None}, lambda: None)
            self.assertEqual(code, 0, access.read_text())
        self.assertEqual(runner.read_json(access), {"diskReadWrite": True})
        private = [
            summary,
            arguments,
            fixture.key,
            fixture.args.output / "console.log",
            fixture.args.output / "results.json",
            fixture.args.output / "storm-aclagent.junit.xml",
            *[
                fixture.args.output / "artifacts" / Path(name).name
                for name in (acl_fixtures.runner.BASE, acl_fixtures.runner.UPDATE)
            ],
        ]
        for path in private:
            self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o600, str(path))
        subprocess.run(
            [
                shutil.which("setpriv"),
                "--reuid",
                str(identity.pw_uid),
                "--regid",
                str(identity.pw_gid),
                "--clear-groups",
                sys.executable,
                "-c",
                "import os,sys\nfrom pathlib import Path\n"
                "for name in sys.argv[1:]:\n"
                " try:\n"
                "  os.listdir(name) if Path(name).is_dir() else Path(name).read_bytes()\n"
                " except PermissionError:\n"
                "  continue\n"
                " raise AssertionError('Private evidence became accessible')\n",
                *map(str, private),
                str(self.args.output),
                str(fixture.args.output),
                str(fixture.args.output / "artifacts"),
            ],
            check=True,
            capture_output=True,
        )
        current = self.root.stat()
        self.assertEqual(
            (current.st_mode, current.st_uid, current.st_gid),
            (ancestor.st_mode, ancestor.st_uid, ancestor.st_gid),
        )

    def test_discovery_accounts_for_unavailable_and_checks_every_selected_input(self):
        self.args.source = RUNNER.parents[2]
        work = self.root / "preflight"
        work.mkdir()
        matrix = dict(fixtures.matrix_entry(name) for name in ("extensions", "base"))

        def command(argv, cwd, log, timeout, grace):
            if log.stem == "install-matrix":
                text = runner.install.MATRIX_PREFIX + json.dumps(matrix)
            else:
                text = runner.legacy.MATRIX_PREFIX + json.dumps(
                    {
                        name: {"configuration": name}
                        for name in ("misc", "base", "extensions")
                    }
                )
            log.write_text(text + "\n")
            return 0

        with patch.object(runner.install, "command", side_effect=command), patch.object(
            runner.legacy, "check_inputs"
        ) as check:
            scenarios, configurations, unavailable = runner.discover(
                self.args,
                {
                    "install": runner.read_json(self.args.install_manifest),
                    "legacy": runner.read_json(self.args.legacy_manifest),
                },
                work,
            )
        self.assertEqual(list(scenarios), ["base_vm-host"])
        self.assertEqual(configurations, ["base"])
        self.assertEqual(check.call_count, len(configurations))
        self.assertEqual(check.call_args.args[-1], "base")
        self.assertEqual(len(unavailable), 3)
        self.assertTrue(all(row["outcome"] == "unavailable" for row in unavailable))

        def completed(rows, args, cancel, save):
            for row in rows:
                row.update(outcome="passed", exitCode=0)
            save()
            return 0

        with patch.object(runner, "verify_inputs", return_value={}), patch.object(
            runner, "preflight", return_value=(scenarios, configurations, unavailable)
        ), patch.object(runner.lifecycle, "qemu_searchable"), patch.object(
            runner, "run_lanes", side_effect=completed
        ):
            self.assertEqual(runner.execute(self.args), 1)
        summary = runner.read_json(self.args.output / "summary.json")
        self.assertTrue(summary["supportedVmAggregatePassed"])
        self.assertFalse(summary["fullAggregateOK"])
        self.assertEqual(summary["outcome"], "incomplete")

    def test_all_manifest_candidates_and_cohorts_checked_before_host_preflight(self):
        self.verify()
        manifest_path = self.args.acl_agent_manifest
        manifest = json.loads(manifest_path.read_text())
        manifest["candidateRevision"] = "c" * 40
        manifest_path.write_text(json.dumps(manifest))
        self.args.acl_agent_manifest_sha256 = runner.install.digest(manifest_path)
        with self.assertRaisesRegex(ValueError, "expected candidate"):
            self.verify()
        with patch.object(
            runner, "verify_inputs", side_effect=ValueError("mismatch")
        ), patch.object(runner, "preflight") as preflight:
            self.assertEqual(runner.execute(self.args), 1)
            preflight.assert_not_called()
        summary = runner.read_json(self.args.output / "summary.json")
        self.assertEqual(summary["outcome"], "unavailable")
        self.assertTrue(
            all(row["outcome"] == "unavailable" for row in summary["lanes"])
        )

    def test_missing_required_lane_input_is_rejected_by_cli(self):
        cli = []
        for name, value in vars(self.args).items():
            cli += ["--" + name.replace("_", "-"), str(value)]
        cli += ["--exclusive-disposable-host"]
        index = cli.index("--acl-agent-manifest")
        del cli[index : index + 2]
        with patch.object(sys, "argv", [str(RUNNER), *cli]), redirect_stderr(
            io.StringIO()
        ), patch.object(runner, "execute") as execute:
            with self.assertRaises(SystemExit) as error:
                runner.main()
            self.assertEqual(error.exception.code, 2)
            execute.assert_not_called()

    def test_wrong_ssh_cohort_and_old_tool_source_fail_closed(self):
        path = self.args.acl_agent_manifest
        manifest = json.loads(path.read_text())
        manifest["producer"]["buildId"] += 1
        path.write_text(json.dumps(manifest))
        self.args.acl_agent_manifest_sha256 = runner.install.digest(path)
        with self.assertRaisesRegex(ValueError, "cohort"):
            self.verify()
        manifest["producer"]["buildId"] -= 1
        path.write_text(json.dumps(manifest))
        self.args.acl_agent_manifest_sha256 = runner.install.digest(path)
        self.args.expected_tool_revision = runner.PRODUCT
        with self.assertRaisesRegex(ValueError, "committed reviewed"):
            self.verify()

    def test_first_process_failure_preserved_with_cleanup_uncertainty_and_later_not_run(
        self,
    ):
        rows = self.rows()
        failed = rows[1]["lane"]
        events = self.fake_commands(rows, failure=failed)
        with patch.object(runner, "evidence", side_effect=[(False, {}), (True, {})]):
            code = runner.run_lanes(rows, self.args, {"signal": None}, lambda: None)
        self.assertEqual(code, 29)
        self.assertEqual(events.read_text().splitlines(), [r["lane"] for r in rows[:2]])
        self.assertTrue(rows[1]["requiresHostDisposal"])
        self.assertEqual(rows[1]["driverExitCode"], 29)
        self.assertTrue(all(r["outcome"] == "not_run" for r in rows[2:]))
        self.assertTrue(
            runner.cleanup_uncertain(
                {
                    "lanes": [
                        {
                            "container": {
                                "requiresHostDisposal": True,
                                "absenceObserved": True,
                            }
                        }
                    ]
                }
            )
        )

    def test_zero_process_exit_without_fresh_evidence_is_not_a_pass(self):
        rows = self.rows()
        self.fake_commands(rows)
        code = runner.run_lanes(rows, self.args, {"signal": None}, lambda: None)
        self.assertEqual(code, 1)
        self.assertTrue(rows[0]["evidenceError"])
        self.assertTrue(rows[0]["requiresHostDisposal"])
        self.assertTrue(all(r["outcome"] == "not_run" for r in rows[1:]))

    def signal_after_ready(self, ready, action):
        errors = []

        def send():
            deadline = time.monotonic() + 5
            while not ready.exists() and time.monotonic() < deadline:
                time.sleep(0.01)
            if not ready.exists():
                errors.append("Child never reached ready state")
                return
            action()

        thread = threading.Thread(target=send)
        thread.start()
        self.addCleanup(thread.join, 6)
        return thread, errors

    def test_repeated_cancellation_allows_real_child_cleanup_and_preserves_signal(self):
        ready, cleaned = self.root / "ready", self.root / "cleaned"
        nested_ready = self.root / "nested-ready"
        nested_cleaned = self.root / "nested-cleaned"
        nested = (
            "import signal,time\nfrom pathlib import Path\n"
            "def stop(sig,frame):\n"
            f" Path({str(nested_cleaned)!r}).touch()\n raise SystemExit(0)\n"
            "signal.signal(signal.SIGTERM, stop)\n"
            f"Path({str(nested_ready)!r}).touch()\n"
            "while True: time.sleep(.05)\n"
        )
        rows = self.rows()
        self.fake_commands(
            rows,
            source=(
                "import signal,time,subprocess,sys\nfrom pathlib import Path\n"
                f"child=subprocess.Popen([sys.executable, '-c', {nested!r}])\n"
                f"while not Path({str(nested_ready)!r}).exists(): time.sleep(.01)\n"
                "def stop(sig,frame):\n"
                " signal.signal(signal.SIGTERM, signal.SIG_IGN)\n"
                " time.sleep(.2)\n child.wait(timeout=.5)\n"
                f" Path({str(cleaned)!r}).touch()\n raise SystemExit(130)\n"
                "signal.signal(signal.SIGTERM, stop)\n"
                f"Path({str(ready)!r}).touch()\n"
                "while True: time.sleep(.05)\n"
            ),
        )

        def send():
            os.kill(os.getpid(), signal.SIGTERM)
            time.sleep(0.1)
            os.kill(os.getpid(), signal.SIGINT)

        with runner.cancellation() as cancel, patch.object(
            runner, "evidence", return_value=(False, {})
        ):
            thread, errors = self.signal_after_ready(ready, send)
            code = runner.run_lanes(rows, self.args, cancel, lambda: None)
            thread.join(6)
        self.assertFalse(errors)
        self.assertTrue(cleaned.exists())
        self.assertTrue(nested_cleaned.exists())
        self.assertEqual(code, 128 + signal.SIGTERM)
        self.assertEqual(rows[0]["driverExitCode"], 130)
        self.assertFalse(rows[0]["forced"])
        self.assertTrue(rows[0]["requiresHostDisposal"])
        self.assertTrue(all(r["outcome"] == "not_run" for r in rows[1:]))

    def test_bounded_force_is_explicit_and_never_green(self):
        ready = self.root / "ready"
        rows = self.rows()
        self.fake_commands(
            rows,
            source=(
                "import signal,time\nfrom pathlib import Path\n"
                "signal.signal(signal.SIGTERM, signal.SIG_IGN)\n"
                f"Path({str(ready)!r}).touch()\n"
                "while True: time.sleep(.05)\n"
            ),
        )
        self.args.shutdown_grace_seconds = 0.1
        cancel = {"signal": None}
        thread, errors = self.signal_after_ready(
            ready, lambda: cancel.update(signal=signal.SIGTERM)
        )
        with patch.object(runner, "evidence", return_value=(False, {})):
            code = runner.run_lanes(rows, self.args, cancel, lambda: None)
        thread.join(6)
        self.assertFalse(errors)
        self.assertEqual(code, 128 + signal.SIGTERM)
        self.assertTrue(rows[0]["forced"])
        self.assertTrue(rows[0]["requiresHostDisposal"])
        self.assertTrue(all(r["outcome"] == "not_run" for r in rows[1:]))


if __name__ == "__main__":
    unittest.main()
