import argparse
import copy
import hashlib
import importlib.util
import io
import json
import os
import sys
import tempfile
import unittest

from contextlib import ExitStack, nullcontext, redirect_stderr, redirect_stdout
from pathlib import Path
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parents[1] / "execute_vm_e2e_install.py"
SPEC = importlib.util.spec_from_file_location("install_execution", SCRIPT)
execution = importlib.util.module_from_spec(SPEC)
with patch.object(sys, "path", [str(SCRIPT.parent), *sys.path]):
    SPEC.loader.exec_module(execution)


class ExecutionTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.args = argparse.Namespace(
            evidence=self.root / "packet.json",
            evidence_sha256="",
            expected_harness_revision="b" * 40,
        )
        self.producer = execution.prepare.producer(
            execution.producer_args(self.args.evidence)
        )
        # Synthetic scaffolding fixture. Never usable as producer evidence.
        self.packet = {
            "schemaVersion": 1,
            "producer": self.producer,
            "review": {"reviewer": "fixture-reviewer", "reference": "fixture-review"},
            "logs": {},
            "rpms": {},
            "artifacts": {},
        }
        self.save_packet()
        self.work = self.root / "private-runtime"
        self.work.mkdir(mode=0o700)
        self.output = self.root / "execution.json"
        self.manifest = {
            "producer": self.producer,
            "lineage": {
                "packetSha256": self.args.evidence_sha256,
                "basis": "independently-reviewed-pinned-evidence",
            },
        }
        self.commands = []
        self.inventory = {"domains": [], "networks": ["baseline"], "pools": []}

    def save_packet(self):
        self.args.evidence.write_bytes(json.dumps(self.packet).encode())
        self.args.evidence_sha256 = hashlib.sha256(
            self.args.evidence.read_bytes()
        ).hexdigest()

    def result(self):
        rows = []
        for config in ("simple", "base", "extensions", "combined", "future"):
            selected = config == "simple"
            rows.append(
                {
                    "scenario": config + "_vm-host",
                    "selected": selected,
                    "outcome": "passed" if selected else "not_run",
                    "variant": {
                        "tridentConfigurationName": config,
                        "installerISOName": "trident-installer",
                        "HARDWARE": "vm",
                        "RUNTIME": "host",
                        "TEST_RING": "pr-e2e",
                    },
                    "cases": (
                        [
                            {"name": name, "outcome": "passed"}
                            for name in sorted(execution.REQUIRED_CASES)
                        ]
                        if selected
                        else []
                    ),
                }
            )
        return {
            "suite": "storm-vm-host-install-configuration",
            "candidateRevision": execution.SOURCE,
            "manifestSha256": "c" * 64,
            "stageType": "pr-e2e",
            "exitCode": 0,
            "outcome": "passed",
            "scenarios": rows,
        }

    def run_command(self, argv, work, name, timeout, **kwargs):
        self.commands.append((argv, work, name, timeout, kwargs))
        if name == "execution.log":
            output = Path(argv[argv.index("--output") + 1])
            output.mkdir()
            (output / "results.json").write_text(json.dumps(self.result()))
        return 0

    def patches(self, stack):
        def patcher(target, *args, **kwargs):
            return stack.enter_context(patch.object(target, *args, **kwargs))

        patcher(execution.os, "geteuid", return_value=0)
        patcher(execution, "REVIEWED_PACKET_SHA256", self.args.evidence_sha256)
        patcher(execution.shutil, "which", return_value="/fixture/tool")
        patcher(execution.tempfile, "mkdtemp", return_value=str(self.work))
        patcher(
            execution.inspection,
            "command",
            side_effect=lambda argv, **kw: (
                self.args.expected_harness_revision
                if argv[:2] == ["git", "rev-parse"]
                else ""
            ),
        )
        self.verify = patcher(
            execution.common, "verify_bundle", return_value=self.manifest
        )
        patcher(execution.common, "digest", return_value="c" * 64)
        patcher(execution.common, "host_lock", return_value=nullcontext())
        patcher(execution.common, "check_host")
        self.inspect_inventory = patcher(
            execution, "inventory", side_effect=lambda: copy.deepcopy(self.inventory)
        )
        self.run = patcher(execution, "run", side_effect=self.run_command)
        stack.enter_context(
            patch.dict(
                os.environ,
                {
                    "EXCLUSIVE_DISPOSABLE_1ES": "true",
                    **{key: "" for key in execution.ALTERNATE_CREDENTIALS},
                },
            )
        )

    def execute(self):
        with ExitStack() as stack:
            self.patches(stack)
            code = execution.execute(self.args, self.output)
        return code, json.loads(self.output.read_text())

    def test_exact_product_packet_harness_and_existing_scenario_invocation(self):
        code, result = self.execute()
        self.assertEqual(code, 0)
        self.assertEqual(result["outcome"], "selected-scenario-passed")
        self.assertEqual(result["sourceRevision"], execution.inspection.REVISION)
        self.assertEqual(result["harnessRevision"], "b" * 40)
        self.assertEqual(result["packetSha256"], self.args.evidence_sha256)
        self.assertTrue(result["resourceInventoryRestored"])
        self.assertEqual(result["unselectedScenarioCount"], 4)
        self.assertEqual(
            result["unselectedConfigurationCounts"],
            {"plain": 1, "registry": 1, "uki": 1, "unknown": 1},
        )
        self.assertEqual(result["caseCounts"]["passed"], 4)
        prepare, setup, run = self.commands
        self.assertEqual(
            prepare[0][1], str(SCRIPT.with_name("prepare_vm_e2e_artifacts.py"))
        )
        for flag, value in (
            ("--build-id", "1266405"),
            ("--expected-definition-id", "5073"),
            ("--expected-revision", execution.SOURCE),
            ("--expected-ref", execution.inspection.REF),
            ("--evidence-sha256", self.args.evidence_sha256),
        ):
            self.assertEqual(prepare[0][prepare[0].index(flag) + 1], value)
        self.assertNotEqual(
            Path(prepare[0][prepare[0].index("--evidence") + 1]), self.args.evidence
        )
        self.assertEqual(
            setup[0], ["bash", str(SCRIPT.with_name("setup-functional.sh"))]
        )
        self.assertEqual(setup[4]["cwd"], SCRIPT.parents[2])
        self.assertEqual(run[0][1], str(SCRIPT.with_name("run-vm-e2e-install.py")))
        self.assertEqual(run[0][run[0].index("--scenario") + 1], "simple_vm-host")
        self.assertEqual(run[0][run[0].index("--stage-type") + 1], "pr-e2e")
        self.assertEqual(run[0][run[0].index("--manifest-sha256") + 1], "c" * 64)
        self.assertEqual(run[2:4], ("execution.log", 3780))
        self.assertEqual(run[4], {"grace": 180})
        self.assertEqual(self.verify.call_count, 2)
        self.assertEqual(
            (self.work / "reviewed-packet.json").read_bytes(),
            self.args.evidence.read_bytes(),
        )

    def test_missing_packet_pin_or_wrong_source_refuses_before_identity_or_installs(
        self,
    ):
        original = self.args.evidence.read_bytes()
        for kind in (
            "missing",
            "no-pin",
            "bad-pin",
            "source",
            "build",
            "ref",
            "review",
        ):
            with self.subTest(kind=kind):
                self.args.evidence.write_bytes(original)
                self.packet = json.loads(original)
                self.save_packet()
                if kind == "missing":
                    self.args.evidence.unlink()
                elif kind == "no-pin":
                    self.args.evidence_sha256 = ""
                elif kind == "bad-pin":
                    self.args.evidence_sha256 = "d" * 64
                else:
                    if kind == "review":
                        self.packet["review"] = {}
                    else:
                        field = {
                            "source": "sourceVersion",
                            "build": "buildId",
                            "ref": "sourceBranch",
                        }[kind]
                        self.packet["producer"][field] = "wrong"
                    self.save_packet()
                code, result = self.execute()
                self.assertNotEqual(code, 0)
                self.assertEqual(result["phase"], "packet")
                self.run.assert_not_called()

    def test_unqualified_preparer_failure_never_reaches_host_installs(self):
        with ExitStack() as stack:
            self.patches(stack)
            self.run.return_value = 1
            self.run.side_effect = None
            code = execution.execute(self.args, self.output)
            self.assertEqual(self.run.call_count, 1)
            self.assertEqual(self.run.call_args.args[2], "prepare.log")
        self.assertNotEqual(code, 0)
        self.assertEqual(json.loads(self.output.read_text())["phase"], "prepare")

    def test_production_packet_pin_cannot_be_replaced_by_hashing_local_claims(self):
        self.assertEqual(
            execution.REVIEWED_PACKET_SHA256,
            "2541fa0bfa52c9b35eae34c22b0e9fa31da57566de4e0b650b44d5991ce3190c",
        )
        with self.assertRaises(ValueError):
            execution.packet(self.args, self.root / "private-packet.json")

    def test_exact_reviewed_packet_handoff_has_independent_fixed_pin(self):
        source = (
            SCRIPT.parents[2]
            / ".github/validation/vm-e2e-install-execution/reviewed-packet-1266405.json"
        )
        raw = source.read_bytes()
        self.assertEqual(len(raw), 21692)
        self.assertEqual(
            hashlib.sha256(raw).hexdigest(), execution.REVIEWED_PACKET_SHA256
        )
        target = self.root / "exact-private-packet.json"
        execution.packet(
            argparse.Namespace(
                evidence=source, evidence_sha256=execution.REVIEWED_PACKET_SHA256
            ),
            target,
        )
        self.assertEqual(target.read_bytes(), raw)

    def test_preflight_harness_missing_dependency_and_ambient_credentials(self):
        for kind in ("harness", "dependency", "credential"):
            with self.subTest(kind=kind), ExitStack() as stack:
                self.patches(stack)
                if kind == "harness":
                    self.args.expected_harness_revision = execution.SOURCE
                elif kind == "dependency":
                    stack.enter_context(
                        patch.object(execution.shutil, "which", return_value=None)
                    )
                elif kind == "credential":
                    stack.enter_context(
                        patch.dict(os.environ, {"AZURE_DEVOPS_EXT_PAT": "fixture"})
                    )
                code = execution.execute(self.args, self.output)
                self.assertNotEqual(code, 0)
                self.run.assert_not_called()
                self.args.expected_harness_revision = "b" * 40

    def test_process_failure_timeout_and_cancellation_stop_and_preserve_status(self):
        for phase in ("prepare.log", "host-setup.log", "execution.log"):
            for failure in (23, 124, 130, KeyboardInterrupt()):
                with (
                    self.subTest(phase=phase, failure=type(failure)),
                    ExitStack() as stack,
                ):
                    self.patches(stack)
                    seen = []

                    def failing(argv, work, name, timeout, **kwargs):
                        seen.append(name)
                        if name == phase:
                            if isinstance(failure, BaseException):
                                raise failure
                            return failure
                        return 0

                    self.run.side_effect = failing
                    code = execution.execute(self.args, self.output)
                    self.assertEqual(
                        code, 130 if isinstance(failure, BaseException) else failure
                    )
                    self.assertEqual(seen[-1], phase)
                    data = json.loads(self.output.read_text())
                    self.assertNotEqual(data["outcome"], "selected-scenario-passed")
                    self.assertEqual(
                        data["hostDisposition"], "dispose-required-no-further-lanes"
                    )

    def test_cleanup_uncertainty_fails_even_after_selected_scenario_pass(self):
        for changed in ({"domains": ["leftover"]}, OSError("private cleanup detail")):
            with self.subTest(changed=type(changed)), ExitStack() as stack:
                self.patches(stack)
                self.inspect_inventory.side_effect = [self.inventory, changed]
                code = execution.execute(self.args, self.output)
                self.assertNotEqual(code, 0)
                data = json.loads(self.output.read_text())
                self.assertFalse(data["resourceInventoryRestored"])
                self.assertEqual(data["outcome"], "failed")

    def test_guest_cases_and_unselected_not_run_cannot_be_overclaimed(self):
        for kind in (
            "missing-guest",
            "not-run",
            "registry",
            "failed",
            "empty",
            "overlimit",
        ):
            with self.subTest(kind=kind):
                value = self.result()
                row = value["scenarios"][0]
                if kind == "missing-guest":
                    row["cases"] = [{"name": "install-vm-deps", "outcome": "passed"}]
                elif kind == "not-run":
                    value["scenarios"][1]["outcome"] = "passed"
                elif kind == "registry":
                    row["variant"]["tridentConfigurationName"] = "extensions"
                elif kind == "failed":
                    row["cases"][0]["outcome"] = "failed"
                elif kind == "empty":
                    value["scenarios"] = []
                else:
                    value["scenarios"] *= 65
                path = self.root / "result.json"
                path.write_text(json.dumps(value))
                result = execution.report()
                result["manifestSha256"] = "c" * 64
                with self.assertRaises((ValueError, KeyError)):
                    execution.summarize(path, result)

    def test_bounded_report_never_exports_raw_fields_or_error_text(self):
        with ExitStack() as stack:
            self.patches(stack)
            self.run.side_effect = OSError("arbitrary-private-console-and-auth-url")
            stdout, stderr = io.StringIO(), io.StringIO()
            with redirect_stdout(stdout), redirect_stderr(stderr):
                self.assertNotEqual(execution.execute(self.args, self.output), 0)
        self.assertEqual(stdout.getvalue() + stderr.getvalue(), "")
        raw = self.output.read_bytes()
        self.assertLessEqual(len(raw), execution.REPORT_LIMIT)
        self.assertNotIn(b"arbitrary-private", raw)
        self.assertEqual(set(json.loads(raw)), set(execution.report()))

    def test_failed_execution_retains_bound_facts_without_exporting_private_text(self):
        private = "fixture-private-detail"

        def failed(argv, work, name, timeout, **kwargs):
            self.run_command(argv, work, name, timeout, **kwargs)
            if name != "execution.log":
                return 0
            directory = Path(argv[argv.index("--output") + 1])
            value = self.result()
            value.update(outcome="failed", exitCode=23, error=private)
            selected = value["scenarios"][0]
            selected["outcome"] = "failed"
            selected["cases"] = [
                {"name": "install-vm-deps", "outcome": "failed"},
                {"name": private, "outcome": "failed"},
                {"name": "install-os", "outcome": "not_run"},
            ]
            (directory / "results.json").write_text(json.dumps(value))
            scenario = directory / execution.SCENARIO
            scenario.mkdir()
            (scenario / "console.log").write_text(
                private + "\nNo matching distribution found\nPermission denied\n"
            )
            (work / name).write_text(private)
            return 23

        with ExitStack() as stack:
            self.patches(stack)
            self.run.side_effect = failed
            stdout, stderr = io.StringIO(), io.StringIO()
            with redirect_stdout(stdout), redirect_stderr(stderr):
                code = execution.execute(self.args, self.output)
        self.assertEqual(code, 23)
        result = json.loads(self.output.read_text())
        self.assertEqual(result["outcome"], "failed")
        self.assertTrue(result["resourceInventoryRestored"])
        self.assertEqual(
            result["caseCounts"],
            {"passed": 0, "failed": 2, "skipped": 0, "not_run": 1},
        )
        diagnostic = result["executionDiagnostic"]
        self.assertEqual(diagnostic["runnerProcessExitCode"], 23)
        self.assertEqual(diagnostic["resultState"], "captured")
        self.assertEqual(diagnostic["selectedOutcome"], "failed")
        self.assertEqual(diagnostic["failedCaseIds"], ["install-vm-deps"])
        self.assertEqual(diagnostic["unrecognizedCaseCount"], 1)
        self.assertEqual(diagnostic["runnerFailureKind"], "unclassified")
        self.assertEqual(
            set(diagnostic["logs"]["scenario"]["markers"]),
            {"pip-no-distribution", "permission-denied"},
        )
        self.assertNotIn(private, self.output.read_text())
        self.assertEqual(stdout.getvalue() + stderr.getvalue(), "")
        self.assertLessEqual(self.output.stat().st_size, execution.REPORT_LIMIT)

    def diagnostic_fixture(self, value):
        directory = self.root / "diagnostic"
        directory.mkdir(exist_ok=True)
        (directory / "results.json").write_text(json.dumps(value))
        result = execution.report()
        result["manifestSha256"] = "c" * 64
        execution.diagnose_execution(directory, 1, result)
        return result

    def test_matrix_failure_is_classified_without_claiming_scenario_execution(self):
        value = self.result()
        value.update(
            scenarios=[],
            exitCode=1,
            outcome="failed",
            error="Storm matrix command failed: 1 fixture-private-detail",
        )
        result = self.diagnostic_fixture(value)
        diagnostic = result["executionDiagnostic"]
        self.assertEqual(diagnostic["resultState"], "captured")
        self.assertEqual(diagnostic["runnerFailureKind"], "matrix-command-failed")
        self.assertEqual(diagnostic["selectedOutcome"], "unknown")
        self.assertIsNone(result["caseCounts"])
        self.assertNotIn("fixture-private-detail", json.dumps(result))
        self.assertEqual(result["outcome"], "failed")

    def test_diagnostic_binding_limits_and_unknown_outcomes_fail_closed(self):
        for kind in ("source", "manifest", "scenario", "cases", "outcome"):
            with self.subTest(kind=kind):
                value = self.result()
                if kind == "source":
                    value["candidateRevision"] = "d" * 40
                elif kind == "manifest":
                    value["manifestSha256"] = "d" * 64
                elif kind == "scenario":
                    value["scenarios"][0]["scenario"] = "other_vm-host"
                elif kind == "cases":
                    value["scenarios"][0]["cases"] *= 129
                else:
                    value["scenarios"][0]["cases"][0]["outcome"] = "unknown"
                result = self.diagnostic_fixture(value)
                self.assertEqual(
                    result["executionDiagnostic"]["resultState"], "invalid"
                )
                self.assertIsNone(result["caseCounts"])
                self.assertEqual(result["executionDiagnostic"]["failedCaseIds"], [])

    def test_missing_or_malformed_diagnostics_never_replace_process_failure(self):
        for code in (23, 124, 130):
            with self.subTest(code=code), ExitStack() as stack:
                self.patches(stack)

                def malformed(argv, work, name, timeout, **kwargs):
                    if name != "execution.log":
                        return 0
                    directory = Path(argv[argv.index("--output") + 1])
                    directory.mkdir(exist_ok=True)
                    (directory / "results.json").write_text("fixture-private-detail")
                    return code

                self.run.side_effect = malformed
                self.assertEqual(execution.execute(self.args, self.output), code)
                result = json.loads(self.output.read_text())
                self.assertEqual(
                    result["executionDiagnostic"]["resultState"], "invalid"
                )
                self.assertEqual(
                    result["executionDiagnostic"]["runnerProcessExitCode"], code
                )
                self.assertNotIn("fixture-private-detail", self.output.read_text())
                self.assertNotEqual(result["outcome"], "selected-scenario-passed")

    def test_log_markers_read_only_bounded_head_tail_and_refuse_symlink(self):
        path = self.root / "private.log"
        path.write_bytes(
            b"Permission denied\n"
            + b"x" * 65536
            + b"address already in use\n"
            + b"x" * 65536
            + b"\nexternally-managed-environment"
        )
        result = execution.log_markers(path)
        self.assertEqual(result["sample"], "head-and-tail")
        self.assertEqual(
            set(result["markers"]), {"permission-denied", "pip-externally-managed"}
        )
        link = self.root / "private-link.log"
        link.symlink_to(path)
        self.assertEqual(
            execution.log_markers(link), {"sample": "unavailable", "markers": []}
        )

    def test_unavailable_symlink_and_oversized_result_diagnostics_are_explicit(self):
        directory = self.root / "diagnostic"
        directory.mkdir()
        path = directory / "results.json"
        for kind in ("missing", "symlink", "oversized"):
            with self.subTest(kind=kind):
                if kind == "symlink":
                    path.symlink_to(self.args.evidence)
                elif kind == "oversized":
                    path.unlink()
                    path.write_bytes(b"x" * (1024 * 1024 + 1))
                result = execution.report()
                execution.diagnose_execution(directory, 23, result)
                diagnostic = result["executionDiagnostic"]
                self.assertEqual(
                    diagnostic["resultState"],
                    "invalid" if kind == "oversized" else "unavailable",
                )
                self.assertEqual(diagnostic["runnerProcessExitCode"], 23)
                self.assertIsNone(result["caseCounts"])
                self.assertEqual(result["outcome"], "failed")

    def test_maximum_case_and_marker_diagnostic_stays_below_report_limit(self):
        value = self.result()
        value["scenarios"][0]["cases"] = [
            {"name": name, "outcome": "failed"}
            for name in sorted(execution.DIAGNOSTIC_CASES)
        ]
        value["scenarios"][0]["cases"] += [
            {"name": "fixture-private-detail", "outcome": "failed"}
        ] * (512 - len(execution.DIAGNOSTIC_CASES))
        directory = self.root / "diagnostic"
        scenario = directory / execution.SCENARIO
        scenario.mkdir(parents=True)
        text = b"\n".join(patterns[0] for patterns in execution.LOG_MARKERS.values())
        for path in (
            self.root / "execution.log",
            directory / "matrix.log",
            scenario / "console.log",
        ):
            path.write_bytes(text)
        result = self.diagnostic_fixture(value)
        execution.publish(self.output, result)
        self.assertLessEqual(self.output.stat().st_size, execution.REPORT_LIMIT)
        self.assertEqual(result["caseCounts"]["failed"], 512)
        self.assertEqual(
            result["executionDiagnostic"]["failedCaseIds"],
            sorted(execution.DIAGNOSTIC_CASES),
        )
        self.assertNotIn("fixture-private-detail", self.output.read_text())

    @unittest.skipUnless(sys.platform == "linux", "requires Linux process groups")
    def test_native_process_failure_and_timeout_use_reviewed_group_reaping(self):
        self.assertEqual(
            execution.run(
                [sys.executable, "-c", "raise SystemExit(17)"],
                self.work,
                "failure.log",
                2,
                group=True,
                grace=0.1,
            ),
            17,
        )
        self.assertEqual(
            execution.run(
                [sys.executable, "-c", "import time; time.sleep(60)"],
                self.work,
                "timeout.log",
                0.1,
                group=True,
                grace=0.1,
            ),
            124,
        )

    def test_bounded_reader_rejects_symlink_and_oversized_input(self):
        self.args.evidence.write_bytes(b"x" * (execution.PACKET_LIMIT + 1))
        with self.assertRaises(ValueError):
            execution.read_json(self.args.evidence, execution.PACKET_LIMIT)
        link = self.root / "link.json"
        link.symlink_to(self.args.evidence)
        with self.assertRaises(OSError):
            execution.read_json(link, execution.PACKET_LIMIT)

    @unittest.skipUnless(sys.platform == "linux", "requires Linux FD protections")
    def test_reuses_pinned_publication_fd_and_never_chowns_substituted_path(self):
        output = self.root / "publication" / "execution.json"
        execution.inspection.prepare_publication(
            output,
            publication_name="execution.json",
            failure_fn=execution.failed_publication,
        )
        args = argparse.Namespace(
            output=output,
            scratch=self.root,
            publication_uid=os.getuid(),
            publication_gid=os.getgid(),
        )
        substitute = self.root / "unrelated.json"
        substitute.write_text("unchanged")

        def replace(fd, scratch, bootstrap_guestfish):
            output.rename(output.with_suffix(".original"))
            output.symlink_to(substitute)
            execution.publish(fd, execution.report())
            return 0

        with (
            patch.dict(os.environ),
            patch.object(execution.inspection.os, "geteuid", return_value=0),
            patch.object(execution.inspection.os, "fchown") as chown,
            redirect_stderr(io.StringIO()),
        ):
            code = execution.inspection.privileged_inspection(
                args,
                execute_fn=replace,
                failure_fn=execution.failed_publication,
                publication_name="execution.json",
            )
        self.assertNotEqual(code, 0)
        self.assertEqual(substitute.read_text(), "unchanged")
        chown.assert_not_called()

    def test_workflow_admission_timeouts_and_exact_upload_allowlist(self):
        workflow = SCRIPT.parents[2] / ".github/workflows/vm-e2e-install-execution.yml"
        text = workflow.read_text()
        self.assertIn("workflow_dispatch:", text)
        self.assertIn("  push:", text)
        self.assertIn("      - user/britel/vm-e2e-install-execution-20261125", text)
        self.assertIn("refs/heads/user/britel/vm-e2e-install-execution-20261125", text)
        self.assertIn("runs-on: trident-github-runner-ubuntu2204-amd64", text)
        self.assertIn("--signal=INT --kill-after=300s 125m", text)
        self.assertIn("cancel-in-progress: false", text)
        self.assertIn("--expected-harness-revision", text)
        self.assertIn("--evidence-sha256", text)
        self.assertIn(
            "--evidence .github/validation/vm-e2e-install-execution/"
            "reviewed-packet-1266405.json",
            text,
        )
        self.assertIn(
            "path: ${{ runner.temp }}/trident-install-report/execution.json", text
        )
        self.assertNotIn("path: ${{ runner.temp }}/trident-install-report\n", text)


if __name__ == "__main__":
    unittest.main()
