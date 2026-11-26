import argparse
import importlib.util
import io
import json
import os
import signal
import stat
import sys
import tempfile
import unittest

from contextlib import ExitStack, contextmanager, redirect_stderr
from pathlib import Path
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parents[1] / "diagnose_runner_umi.py"
SPEC = importlib.util.spec_from_file_location("umi_diagnostic", SCRIPT)
diagnostic = importlib.util.module_from_spec(SPEC)
with patch.object(sys, "path", [str(SCRIPT.parent), *sys.path]):
    SPEC.loader.exec_module(diagnostic)


class UmiDiagnosticTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.home = self.root / "private-home"
        self.home.mkdir(mode=0o700)
        self.output = self.root / "diagnostic.json"
        self.report = diagnostic.new_report()

    def fixture(self, login=1):
        return [
            (
                {"state": "ok", "exitCode": 0, "exceptionClass": None},
                b'{"azure-cli":"2.75.0","extensions":{"fixture-private":"private"}}',
                b"",
            ),
            (
                {"state": "ok", "exitCode": 0, "exceptionClass": None},
                b"    --identity -i : login\n    --client-id : id\n"
                b"    --allow-no-subscriptions : yes\n    --output -o : json\n",
                b"",
            ),
            (
                {
                    "state": "ok" if login == 0 else "failed",
                    "exitCode": login,
                    "exceptionClass": None,
                },
                b"fixture-private ignored stdout",
                b"ERROR: (InvalidAuthenticationTokenTenant) fixture-private\nStatus Code: 401\n",
            ),
        ]

    def environment(self):
        return {
            **{name: "" for name in diagnostic.ENVIRONMENT},
            "HOME": str(self.home),
            "AZURE_CONFIG_DIR": str(self.home / ".azure"),
            "AZURE_EXTENSION_DIR": "fixture-installed-extension",
            "IDENTITY_ENDPOINT": "fixture-private",
            "HTTPS_PROXY": "fixture-private",
            "EXCLUSIVE_DISPOSABLE_1ES": "true",
        }

    def execute(self, rows):
        with (
            patch.dict(os.environ, self.environment()),
            patch.object(diagnostic.os, "geteuid", return_value=0),
            patch.object(diagnostic.shutil, "which", return_value="/fixture/az"),
            patch.object(diagnostic, "capture", side_effect=rows) as capture,
        ):
            code = diagnostic.execute(self.output, self.root, self.report)
        return code, json.loads(self.output.read_text()), capture

    def test_exact_three_commands_one_login_and_presence_only_report(self):
        code, report, capture = self.execute(self.fixture())
        self.assertEqual(code, 1)
        self.assertEqual(
            [call.args for call in capture.call_args_list],
            [
                (["az", "version", "--output", "json"], 30),
                (["az", "login", "--help"], 30),
                (diagnostic.LOGIN, 120),
            ],
        )
        self.assertEqual(
            diagnostic.LOGIN,
            [
                "az",
                "login",
                "--identity",
                "--client-id",
                diagnostic.CLIENT_ID,
                "--allow-no-subscriptions",
                "--output",
                "none",
            ],
        )
        self.assertEqual(report["cliVersion"], "2.75.0")
        self.assertTrue(all(report["argumentSupport"].values()))
        self.assertTrue(report["environmentPresence"]["IDENTITY_ENDPOINT"])
        self.assertTrue(report["environmentPresence"]["HTTPS_PROXY"])
        self.assertFalse(report["environmentPresence"]["MSI_ENDPOINT"])
        self.assertTrue(
            all(type(value) is bool for value in report["environmentPresence"].values())
        )
        self.assertEqual(report["loginFailureMarkers"], ["token-issuer-mismatch"])
        self.assertEqual(report["reportedHttpStatus"], 401)
        self.assertNotIn("fixture-private", self.output.read_text())
        self.assertNotIn("fixture-installed-extension", self.output.read_text())

    def test_success_is_login_only_not_guest_or_attachment_proof(self):
        code, report, capture = self.execute(self.fixture(login=0))
        self.assertEqual(code, 0)
        self.assertEqual(report["status"], "login-succeeded-not-guest-execution")
        self.assertEqual(capture.call_count, 3)
        self.assertEqual(report["loginFailureMarkers"], [])
        self.assertIsNone(report["reportedHttpStatus"])

    def test_publication_error_after_login_success_still_fails_diagnostic(self):
        publish = diagnostic.publish
        interrupted = False

        def checkpoint(output, report):
            nonlocal interrupted
            if (
                report["status"] == "login-succeeded-not-guest-execution"
                and not interrupted
            ):
                interrupted = True
                raise PermissionError("fixture-private")
            publish(output, report)

        with patch.object(diagnostic, "publish", side_effect=checkpoint):
            code, report, capture = self.execute(self.fixture(login=0))
        self.assertEqual(code, 1)
        self.assertEqual(report["status"], "failed")
        self.assertEqual(report["exceptionClass"], "PermissionError")
        self.assertEqual(report["commands"]["login"]["state"], "ok")
        self.assertEqual(capture.call_count, 3)
        self.assertNotIn("fixture-private", self.output.read_text())

    def test_missing_cli_stops_without_install_or_login(self):
        with (
            patch.dict(os.environ, self.environment()),
            patch.object(diagnostic.os, "geteuid", return_value=0),
            patch.object(diagnostic.shutil, "which", return_value=None),
            patch.object(diagnostic, "capture") as capture,
        ):
            self.assertEqual(diagnostic.execute(self.output, self.root, self.report), 1)
        capture.assert_not_called()
        self.assertFalse(self.report["preflight"]["azureCli"])
        self.assertEqual(self.report["commands"]["login"]["state"], "not-run")

    def test_unknown_version_help_and_native_error_remain_explicit(self):
        for raw in (b"[]", b'{"azure-cli":"fixture-private"}', b"[" * 2000, None):
            with self.subTest(raw=raw):
                self.report = diagnostic.new_report()
                rows = self.fixture()
                rows[0] = (rows[0][0], raw, b"")
                rows[1] = (rows[1][0], None, b"")
                rows[2] = (rows[2][0], b"", b"fixture-private https://private/401")
                code, report, _ = self.execute(rows)
                self.assertEqual(code, 1)
                self.assertIsNone(report["cliVersion"])
                self.assertIsNone(report["argumentSupport"])
                self.assertEqual(report["loginFailureMarkers"], ["unclassified"])
                self.assertIsNone(report["reportedHttpStatus"])
                self.assertNotIn("fixture-private", self.output.read_text())

    def test_help_requires_option_declaration_not_a_mention_or_url(self):
        rows = self.fixture()
        rows[1] = (
            rows[1][0],
            b"Example: --client-id private\nhttps://private/--identity\n  --output -o : json\n",
            b"",
        )
        _, report, _ = self.execute(rows)
        self.assertEqual(
            report["argumentSupport"],
            {
                "identity": False,
                "client-id": False,
                "allow-no-subscriptions": False,
                "output": True,
            },
        )

    def test_literal_provider_markers_and_http_status_never_copy_payloads(self):
        for raw, marker, status in (
            (
                b'ERROR: {"error":"invalid_request","error_description":"Identity not found","access_token":"fixture-private"}',
                "identity-not-found",
                None,
            ),
            (
                b"ERROR: unrecognized arguments: --client-id fixture-private",
                "unsupported-login-argument",
                None,
            ),
            (
                b"ERROR: Failed to connect to MSI. fixture-private\nGet Token request returned http error: 400, reason: Bad Request",
                "msi-connection-failed",
                400,
            ),
            (
                b"ERROR: (AuthorizationFailed) fixture-private\nStatus Code: 403",
                "arm-authorization-failed",
                403,
            ),
            (
                b"ERROR: AADSTS700016: fixture-private",
                "entra-application-not-found",
                None,
            ),
            (
                b'{"error":"invalid_client","error_description":"AADSTS90002: fixture-private"}',
                "entra-tenant-not-found",
                None,
            ),
            (
                b"ERROR: requests.exceptions.ProxyError: fixture-private",
                "proxy-error",
                None,
            ),
        ):
            with self.subTest(marker=marker):
                markers, actual = diagnostic.login_observations(raw)
                self.assertIn(marker, markers)
                self.assertEqual(actual, status)
                self.assertNotIn("fixture-private", json.dumps([markers, actual]))
        for raw in (
            None,
            b"HTTP 503 fixture-private",
            b"https://private/Status Code: 401",
            b'{"access_token":"Identity not found","url":"HTTPError: 401"}',
            b'{"error":' + b"[" * 2000,
            b"x" * (diagnostic.PRIVATE_LIMIT + 1),
        ):
            self.assertEqual(
                diagnostic.login_observations(raw), (["unclassified"], None)
            )
        self.assertEqual(
            diagnostic.login_observations(b"Status Code: 401\nStatus Code: 403")[1],
            None,
        )

    def test_timeout_and_cancellation_stop_all_later_commands(self):
        for index in range(3):
            for state, code in (("timeout", 124), ("cancelled", 130)):
                self.report = diagnostic.new_report()
                rows = self.fixture()
                rows[index] = (
                    {"state": state, "exitCode": code, "exceptionClass": None},
                    None,
                    None,
                )
                actual, report, capture = self.execute(rows)
                self.assertEqual(actual, code)
                self.assertEqual(report["status"], state)
                self.assertEqual(capture.call_count, index + 1)

    def test_execute_restores_callers_handlers_on_every_command_outcome(self):
        signals = (signal.SIGINT, signal.SIGTERM)
        original = {sig: signal.getsignal(sig) for sig in signals}

        def caller(signum, frame):
            raise diagnostic.inspection.Cancelled()

        try:
            for sig in signals:
                signal.signal(sig, caller)
            for state in ("ok", "failed", "timeout", "cancelled"):
                with self.subTest(state=state):
                    self.report = diagnostic.new_report()
                    rows = self.fixture(login=0 if state == "ok" else 1)
                    if state in ("timeout", "cancelled"):
                        rows[0] = (
                            {
                                "state": state,
                                "exitCode": 124 if state == "timeout" else 130,
                                "exceptionClass": None,
                            },
                            None,
                            None,
                        )
                    self.execute(rows)
                    for sig in signals:
                        self.assertIs(signal.getsignal(sig), caller)
            with patch.object(
                diagnostic, "publish", side_effect=PermissionError("fixture-private")
            ):
                with self.assertRaises(PermissionError):
                    self.execute(self.fixture(login=0))
            for sig in signals:
                self.assertIs(signal.getsignal(sig), caller)
        finally:
            for sig, handler in original.items():
                signal.signal(sig, handler)

    def test_main_cancellation_during_private_profile_cleanup_fails_report(self):
        output = self.root / "cleanup-cancel" / "diagnostic.json"
        diagnostic.inspection.prepare_publication(
            output,
            publication_name="diagnostic.json",
            failure_fn=diagnostic.failed_publication,
        )
        temporary = diagnostic.tempfile.TemporaryDirectory
        homes = []

        @contextmanager
        def cancel_cleanup(*args, **kwargs):
            with temporary(*args, **kwargs) as home:
                homes.append(Path(home))
                (Path(home) / "fixture-cache").write_bytes(b"fixture-private")
                yield home
                signal.getsignal(signal.SIGTERM)(signal.SIGTERM, None)

        argv = [
            str(SCRIPT),
            "--output",
            str(output),
            "--scratch",
            str(self.root),
            "--publication-uid",
            str(os.getuid()),
            "--publication-gid",
            str(os.getgid()),
        ]
        original = {
            sig: signal.getsignal(sig) for sig in (signal.SIGINT, signal.SIGTERM)
        }
        umask = os.umask(0o077)
        try:
            with (
                patch.object(sys, "argv", argv),
                patch.dict(os.environ, self.environment()),
                patch.object(diagnostic.os, "geteuid", return_value=0),
                patch.object(diagnostic.os, "fchown"),
                patch.object(diagnostic.shutil, "which", return_value="/fixture/az"),
                patch.object(diagnostic, "capture", side_effect=self.fixture(login=0)),
                patch.object(
                    diagnostic.tempfile,
                    "TemporaryDirectory",
                    side_effect=cancel_cleanup,
                ),
            ):
                code = diagnostic.main()
        finally:
            os.umask(umask)
            for sig, handler in original.items():
                signal.signal(sig, handler)
        self.assertEqual(code, 130)
        report = json.loads(output.read_text())
        self.assertEqual(report["status"], "failed")
        self.assertEqual(report["publicationFailure"], "cancelled-context")
        self.assertEqual(report["commands"]["login"]["state"], "ok")
        self.assertTrue(homes)
        self.assertTrue(all(not home.exists() for home in homes))
        self.assertNotIn("fixture-private", output.read_text())

    def test_native_capture_bounds_output_and_retains_spawn_failure(self):
        with patch.dict(os.environ, self.environment()):
            row, stdout, stderr = diagnostic.capture(
                [
                    sys.executable,
                    "-c",
                    "import sys; print('fixture-private'); sys.stderr.write('x'*65537); sys.exit(7)",
                ],
                5,
            )
            self.assertEqual(row["exitCode"], 7)
            self.assertIn(b"fixture-private", stdout)
            self.assertIsNone(stderr)
            row, stdout, stderr = diagnostic.capture(
                [str(self.root / "missing-executable")], 5
            )
        self.assertEqual(row["exceptionClass"], "FileNotFoundError")
        self.assertIsNone(stdout)
        self.assertIsNone(stderr)

    def test_native_timeout_and_cancel_reap_before_returning(self):
        popen = diagnostic.subprocess.Popen
        wait = popen.wait
        for cancelled in (False, True):
            processes = []
            interrupted = False

            def spawn(*args, **kwargs):
                self.assertEqual(kwargs["env"]["PYTHONDONTWRITEBYTECODE"], "1")
                self.assertEqual(kwargs["env"]["IDENTITY_ENDPOINT"], "fixture-private")
                process = popen(*args, **kwargs)
                processes.append(process)
                return process

            def cancel_once(process, *args, **kwargs):
                nonlocal interrupted
                if not interrupted:
                    interrupted = True
                    raise KeyboardInterrupt()
                return wait(process, *args, **kwargs)

            with ExitStack() as stack:
                stack.enter_context(patch.dict(os.environ, self.environment()))
                stack.enter_context(
                    patch.object(diagnostic.subprocess, "Popen", side_effect=spawn)
                )
                if cancelled:
                    stack.enter_context(patch.object(popen, "wait", cancel_once))
                row, _, _ = diagnostic.capture(
                    [sys.executable, "-c", "import time; time.sleep(60)"], 0.05
                )
            self.assertEqual(row["exitCode"], 130 if cancelled else 124)
            self.assertIsNotNone(processes[0].poll())

    def test_protected_root_profile_cleanup_and_replaced_publication(self):
        ambient = self.home / ".azure"
        ambient.mkdir()
        sentinel = ambient / "config"
        sentinel.write_bytes(b"fixture-ambient-do-not-touch")
        for replace in (False, True):
            output = self.root / str(replace) / "diagnostic.json"
            report = diagnostic.new_report()
            diagnostic.inspection.prepare_publication(
                output,
                publication_name="diagnostic.json",
                failure_fn=diagnostic.failed_publication,
            )
            args = argparse.Namespace(
                output=output,
                scratch=self.root,
                publication_uid=os.getuid(),
                publication_gid=os.getgid(),
            )
            homes = []
            rows = iter(self.fixture(login=0))
            moved = output.with_name("original.json")

            def capture(*args):
                home = Path(os.environ["HOME"])
                homes.append(home)
                self.assertNotEqual(home, self.home)
                self.assertEqual(stat.S_IMODE(home.stat().st_mode), 0o700)
                self.assertEqual(Path(os.environ["AZURE_CONFIG_DIR"]).parent, home)
                config = Path(os.environ["AZURE_CONFIG_DIR"])
                config.mkdir(exist_ok=True)
                (config / "fixture-cache").write_bytes(b"fixture-private")
                if replace and not moved.exists():
                    output.rename(moved)
                    output.write_bytes(b"fixture-do-not-touch")
                return next(rows)

            private = io.StringIO()
            with (
                patch.dict(os.environ, self.environment()),
                patch.object(diagnostic.os, "geteuid", return_value=0),
                patch.object(diagnostic.os, "fchown"),
                patch.object(diagnostic.shutil, "which", return_value="/fixture/az"),
                patch.object(diagnostic, "capture", side_effect=capture),
                redirect_stderr(private),
            ):
                code = diagnostic.inspection.privileged_inspection(
                    args,
                    publication_name="diagnostic.json",
                    execute_fn=lambda fd, scratch, bootstrap_guestfish: diagnostic.execute(
                        fd, scratch, report
                    ),
                    failure_fn=lambda fd, fact, **kwargs: diagnostic.failed_publication(
                        fd, fact, report, **kwargs
                    ),
                    retain_secondary_failure=True,
                )
            self.assertEqual(code, 1 if replace else 0)
            self.assertTrue(all(not home.exists() for home in homes))
            self.assertEqual(sentinel.read_bytes(), b"fixture-ambient-do-not-touch")
            if replace:
                self.assertEqual(output.read_bytes(), b"fixture-do-not-touch")
                self.assertEqual(
                    json.loads(moved.read_text())["publicationFailure"],
                    "publication-finalize-failed",
                )
            self.assertNotIn("fixture-private", private.getvalue())

    def test_report_bound_includes_all_fixed_observations(self):
        report = diagnostic.new_report()
        report.update(
            environmentPresence={name: False for name in diagnostic.ENVIRONMENT},
            argumentSupport={name: False for name in diagnostic.ARGUMENTS},
            preflight={
                name: False
                for name in ("linux", "root", "exclusiveDisposable", "azureCli")
            },
            exceptionClass="ConnectionAbortedError",
            commands={
                name: {
                    "state": "cancelled",
                    "exitCode": -128,
                    "exceptionClass": "ConnectionAbortedError",
                }
                for name in ("version", "help", "login")
            },
            loginFailureMarkers=sorted(
                set(diagnostic.ERROR_CODES.values())
                | set(diagnostic.MARKERS)
                | set(diagnostic.AADSTS.values())
            ),
            reportedHttpStatus=503,
            cliVersion="999.999.999",
            publicationFailure="publication-finalize-failed",
        )
        diagnostic.publish(self.output, report)
        self.assertLess(self.output.stat().st_size, diagnostic.REPORT_LIMIT)
        raw = self.output.read_bytes()
        report["status"] = "x" * diagnostic.REPORT_LIMIT
        with self.assertRaises(diagnostic.inspection.Failure):
            diagnostic.publish(self.output, report)
        self.assertEqual(self.output.read_bytes(), raw)

    def test_workflow_owns_separate_branch_and_uploads_only_one_report(self):
        workflow = (
            SCRIPT.parents[2] / ".github/workflows/vm-e2e-umi-diagnostic.yml"
        ).read_text()
        branch = "user/britel/vm-e2e-umi-diagnostic-20261125"
        self.assertIn("branches:\n      - " + branch, workflow)
        self.assertIn("if: github.ref == 'refs/heads/" + branch + "'", workflow)
        self.assertIn("runs-on: trident-github-runner-ubuntu2204-amd64", workflow)
        self.assertIn(
            "path: ${{ runner.temp }}/runner-umi-report/diagnostic.json", workflow
        )
        self.assertIn("timeout --signal=INT --kill-after=30s 5m", workflow)
        self.assertIn("persist-credentials: false", workflow)
        self.assertNotIn("prepare_vm_e2e_artifacts", workflow)
        existing = (
            SCRIPT.parents[2] / ".github/workflows/vm-e2e-install-execution.yml"
        ).read_text()
        self.assertNotIn(branch, existing)


if __name__ == "__main__":
    unittest.main()
