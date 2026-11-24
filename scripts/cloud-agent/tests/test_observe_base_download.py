import hashlib
import importlib.util
import json
import os
import signal
import subprocess
import sys
import tempfile
import textwrap
import unittest

from pathlib import Path
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parents[1] / "observe_base_download.py"
SPEC = importlib.util.spec_from_file_location("base_observer", SCRIPT)
observer = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(observer)

INSPECT_SPEC = importlib.util.spec_from_file_location(
    "inspect_images", SCRIPT.with_name("inspect_vm_e2e_images.py")
)
inspector = importlib.util.module_from_spec(INSPECT_SPEC)
with patch.object(sys, "path", [str(SCRIPT.parent), *sys.path]):
    INSPECT_SPEC.loader.exec_module(inspector)


@unittest.skipUnless(sys.platform == "linux", "requires native Unix private child FDs")
class ChildObserverTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.extension = self.root / "extensions/azure-devops"
        self.audit = self.root / "entrypoint.json"
        self.trace_audit = self.root / "trace.json"
        self.contracts = {}
        self.module(
            "azext_devops.dev.common.artifacttool_updater",
            """
            import io
            import zipfile
            MODE = ""
            class Response:
                def __init__(self):
                    self.status_code = 200 if MODE != "untyped" else "fixture-sensitive"
                    self.headers = {} if MODE in ("length-missing", "length-suppressed") else {"Content-Length": "1"}
            class Spinner:
                def __enter__(self):
                    return self
                def __exit__(self, kind, error, traceback):
                    return MODE == "length-suppressed"
            def _get_current_release(organization, override_version):
                if MODE == "null-release":
                    return None
                return ("fixture-private-uri", "fixture-private-release")
            def _update_artifacttool(uri, release_id):
                with Spinner():
                    response = Response()
                    content_length_header = response.headers["Content-Length"].strip()
                    if MODE == "length-invalid":
                        content_length_header = "fixture-sensitive"
                    content_length = int(content_length_header)
                    content = io.BytesIO()
                    if MODE != "bad-zip":
                        with zipfile.ZipFile(content, "w") as archive:
                            archive.writestr("fixture", b"never executed")
                    f = zipfile.ZipFile(content)
                    artifacttool_binary = "fixture-private-path"
                    if MODE != "no-executable":
                        artifacttool_stat = object()
                    release_dir = "fixture-private-directory"
                    f.close()
            class ArtifactToolUpdater:
                def _get_artifacttool(self, organization):
                    release = _get_current_release(organization, None)
                    release_uri, release_id = release
                    _update_artifacttool(release_uri, release_id)
            """,
            {
                "_get_current_release": "release-lookup",
                "_update_artifacttool": "tool-download-extract",
                "ArtifactToolUpdater._get_artifacttool": "tool-bootstrap",
            },
        )
        # Preserve the exact production Popen assignment line contract.
        external = (
            "import subprocess\nimport sys\nclass ExternalToolInvoker:\n"
            "    def start(self, command_args, env):\n"
            + "\n" * 28
            + "        self._proc = subprocess.Popen(command_args, env=env)\n"
            "    def wait(self):\n"
            "        self._proc.wait()\n"
            "        if self._proc.returncode:\n"
            "            raise ValueError('fixture-sensitive-inner-wrapper')\n"
            "        return self._proc\n"
        )
        self.assertEqual(
            external.splitlines()[32].strip().split(" = ")[0], "self._proc"
        )
        self.module(
            "azext_devops.dev.common.external_tool",
            external,
            {
                "ExternalToolInvoker.start": "tool-spawn",
                "ExternalToolInvoker.wait": "tool-wait",
            },
        )
        self.module(
            "azext_devops.dev.common.services",
            """
            class Sensitive:
                def __getattribute__(self, name):
                    raise AssertionError("fixture-sensitive-credential-inspected")
                def __repr__(self):
                    raise AssertionError("fixture-sensitive-credential-formatted")
            def _get_credentials(organization):
                return Sensitive()
            """,
            {"_get_credentials": "credential-resolution"},
        )
        self.module(
            "azext_devops.devops_sdk.client",
            """
            class Client:
                def _handle_error(self, request, response):
                    raise ValueError("fixture-sensitive-server-string")
            """,
            {"Client._handle_error": "sdk-http-error"},
        )
        entrypoint = self.module(
            "azure.cli.__main__",
            f"""
            import json
            import os
            import sys
            import time
            from pathlib import Path
            from azext_devops.dev.common import artifacttool_updater as updater
            from azext_devops.dev.common.external_tool import ExternalToolInvoker
            from azext_devops.dev.common.services import _get_credentials
            from azext_devops.devops_sdk.client import Client
            Path({str(self.audit)!r}).write_text(json.dumps({{
                "argv": sys.argv[1:], "isolated": sys.flags.isolated,
                "installer": os.environ.get("AZ_INSTALLER"),
            }}))
            updater.MODE = sys.argv[-1]
            try:
                _get_credentials("fixture")
                if updater.MODE == "late-child" and os.fork() == 0:
                    time.sleep(1)
                    raise SystemExit(77)
                if updater.MODE == "timeout":
                    time.sleep(30)
                if updater.MODE == "http-error":
                    response = updater.Response()
                    response.status_code = 403
                    Client()._handle_error(None, response)
                if updater.MODE == "length-recovered":
                    updater.MODE = "length-missing"
                    try:
                        updater._update_artifacttool("fixture", "fixture")
                    except KeyError:
                        updater.MODE = "normal"
                updater.ArtifactToolUpdater()._get_artifacttool("fixture")
                invoker = ExternalToolInvoker()
                executable = "/fixture-absent" if updater.MODE == "spawn-failed" else sys.executable
                inner = 7 if updater.MODE == "inner-failed" else 0
                invoker.start([executable, "-c", "raise SystemExit(" + str(inner) + ")"], os.environ.copy())
                invoker.wait()
                if updater.MODE == "fd-probe":
                    probe = "import os;\\ntry: os.fstat(" + str(updater.CHANNEL_FD) + ")\\nexcept OSError: raise SystemExit(0)\\nraise SystemExit(23)"
                    invoker = ExternalToolInvoker()
                    invoker.start([sys.executable, "-c", probe], os.environ.copy())
                    invoker.wait()
            except Exception:
                sys.stderr.write("fixture-sensitive-token https://fixture.invalid/?secret")
                raise SystemExit(17)
            raise SystemExit(17 if updater.MODE == "late-child" else 0)
            """,
        )
        self.cli_hash = hashlib.sha256(entrypoint.read_bytes()).hexdigest()
        self.bootstrap = self.root / "bootstrap.py"
        self.bootstrap.write_text(
            textwrap.dedent(
                f"""
                import importlib.util
                import json
                import sys
                from pathlib import Path
                sys.path.insert(0, {str(self.extension)!r})
                spec = importlib.util.spec_from_file_location("child_observer", {str(SCRIPT)!r})
                observer = importlib.util.module_from_spec(spec)
                spec.loader.exec_module(observer)
                observer.CONTRACTS = {self.contracts!r}
                observer.CLI_SHA256 = {self.cli_hash!r}
                import azext_devops.dev.common.artifacttool_updater as updater
                updater.CHANNEL_FD = int(sys.argv[1])
                try:
                    observer.main()
                finally:
                    Path({str(self.trace_audit)!r}).write_text(json.dumps(sys.gettrace() is None))
                """
            )
        )
        self.az = self.root / "original-az"
        self.az.write_text("#!/bin/sh\nexit 29\n")
        self.az.chmod(0o700)

    def module(self, name, body, functions=None):
        path = self.extension / (name.replace(".", "/") + ".py")
        path.parent.mkdir(parents=True, exist_ok=True)
        parent = path.parent
        while parent != self.extension:
            (parent / "__init__.py").touch()
            parent = parent.parent
        path.write_text(textwrap.dedent(body))
        if functions is not None:
            self.contracts[name.removeprefix("azext_devops.")] = (
                hashlib.sha256(path.read_bytes()).hexdigest(),
                functions,
            )
        return path

    def wrapped(self, argv, fd):
        return [sys.executable, "-I", str(self.bootstrap), str(fd), *argv]

    def child(self, mode):
        reader, writer = os.pipe2(os.O_NONBLOCK | os.O_CLOEXEC)
        try:
            process = subprocess.Popen(
                self.wrapped(
                    [str(self.az), "artifacts", "universal", "download", mode], writer
                ),
                pass_fds=(writer,),
                env={**os.environ, "AZURE_EXTENSION_DIR": str(self.extension.parent)},
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                start_new_session=True,
            )
            try:
                process.communicate(timeout=5)
                result = observer.read_channel(reader)
            finally:
                inspector.VM_RUNNER.signal_group(process, signal.SIGKILL)
                process.wait(timeout=5)
        finally:
            os.close(writer)
            os.close(reader)
        self.assertNotIn("fixture-sensitive", json.dumps(result))
        self.assertNotIn("fixture-private", json.dumps(result))
        self.assertNotIn("fixture.invalid", json.dumps(result))
        self.assertNotIn(str(self.root), json.dumps(result))
        return process.returncode, result

    def test_real_boundaries_and_unchanged_entrypoint_arguments_exit_and_isolation(
        self,
    ):
        code, result = self.child("normal")
        self.assertEqual(code, 0)
        self.assertEqual(result["status"], "available")
        for key in observer.BOOLEANS:
            self.assertIs(result[key], True)
        self.assertEqual(result["innerExitCode"], 0)
        self.assertTrue(result["phases"]["credential-resolution"]["returned"])
        self.assertIsNone(result["phases"]["sdk-http-error"])
        self.assertEqual(result["phases"]["tool-download-extract"]["httpStatus"], 200)
        self.assertEqual(
            json.loads(self.audit.read_text()),
            {
                "argv": ["artifacts", "universal", "download", "normal"],
                "isolated": 1,
                "installer": "DEB",
            },
        )
        self.assertTrue(json.loads(self.trace_audit.read_text()))
        self.assertIsNone(sys.gettrace())

    def test_distinguishing_null_release_length_zip_and_missing_executable(self):
        for mode, key, phase, kind in (
            ("null-release", "releasePresent", "tool-bootstrap", "TypeError"),
            (
                "length-missing",
                "contentLengthValid",
                "tool-download-extract",
                "KeyError",
            ),
            (
                "length-invalid",
                "contentLengthValid",
                "tool-download-extract",
                "ValueError",
            ),
            ("bad-zip", "zipOpened", "tool-download-extract", "BadZipFile"),
            ("no-executable", "executablePresent", None, None),
            ("spawn-failed", "spawnSucceeded", "tool-spawn", "FileNotFoundError"),
        ):
            with self.subTest(mode=mode):
                code, result = self.child(mode)
                self.assertEqual(code, 0 if mode == "no-executable" else 17)
                self.assertIs(result[key], False)
                if phase:
                    self.assertEqual(result["phases"][phase]["exceptionKind"], kind)
                if mode == "null-release":
                    self.assertIsNone(result["contentLengthValid"])
                    self.assertIsNone(result["spawnSucceeded"])

    def test_typed_http_and_inner_exit_are_not_outer_exit_or_server_text(self):
        code, result = self.child("http-error")
        self.assertEqual(code, 17)
        self.assertEqual(result["phases"]["sdk-http-error"]["httpStatus"], 403)
        self.assertIsNone(result["serviceCode"])
        self.assertIsNone(result["innerExitCode"])
        code, result = self.child("inner-failed")
        self.assertEqual(code, 17)
        self.assertEqual(result["innerExitCode"], 7)
        code, result = self.child("untyped")
        self.assertEqual(code, 0)
        self.assertIsNone(result["phases"]["tool-download-extract"]["httpStatus"])

    def test_contextmanager_cleanup_does_not_prove_recovery_or_replace_outer_exit(self):
        for mode, expected_exit in (("length-missing", 17), ("length-suppressed", 0)):
            with self.subTest(mode=mode):
                code, result = self.child(mode)
                self.assertEqual(code, expected_exit)
                phase = result["phases"]["tool-download-extract"]
                self.assertEqual(phase["exceptionKind"], "KeyError")
                self.assertIsNone(phase["returned"])
                self.assertIs(result["contentLengthValid"], False)
                self.assertIsNone(result["zipOpened"])
                bootstrap = result["phases"]["tool-bootstrap"]
                if expected_exit:
                    self.assertEqual(bootstrap["exceptionKind"], "KeyError")
                    self.assertIsNone(bootstrap["returned"])
                else:
                    self.assertIsNone(bootstrap["exceptionKind"])
                    self.assertIs(bootstrap["returned"], True)

    def test_normal_return_after_a_new_call_is_not_tainted_by_previous_exception(self):
        code, result = self.child("length-recovered")
        self.assertEqual(code, 0)
        phase = result["phases"]["tool-download-extract"]
        self.assertEqual(phase["exceptionKind"], "KeyError")
        self.assertIs(phase["returned"], True)

    def test_changed_source_falls_back_once_to_original_entrypoint(self):
        for relative, gate in (
            (
                "azext_devops/dev/common/external_tool.py",
                "extension-external-tool-source",
            ),
            ("azext_devops/devops_sdk/client.py", "extension-sdk-client-source"),
            ("azure/cli/__main__.py", "cli-main-source"),
        ):
            with self.subTest(gate=gate):
                path = self.extension / relative
                body = path.read_bytes()
                changed = body + b"\n# altered fixture\n"
                path.write_bytes(changed)
                try:
                    code, result = self.child("normal")
                    self.assertEqual(code, 29)
                    self.assertEqual(
                        result,
                        observer.rejected(
                            gate,
                            "hash-mismatch",
                            hashlib.sha256(changed).hexdigest(),
                        ),
                    )
                    self.assertFalse(self.audit.exists())
                finally:
                    path.write_bytes(body)

    def test_runtime_code_mismatch_disables_observation_without_changing_cli_exit(self):
        body = self.bootstrap.read_text()
        self.bootstrap.write_text(
            body.replace(
                "observer.main()",
                "import azext_devops.dev.common.artifacttool_updater as updater\n"
                "    updater._get_current_release = lambda *args: None\n"
                "    observer.main()",
            )
        )
        code, result = self.child("normal")
        self.assertEqual(code, 17)
        self.assertEqual(
            result, observer.rejected("release-lookup-function", "binding-mismatch")
        )

    def test_missing_source_and_entrypoint_discovery_have_fixed_distinct_gates(self):
        for relative, gate, reason in (
            (
                "azext_devops/dev/common/external_tool.py",
                "extension-external-tool-source",
                "unreadable",
            ),
            ("azure/cli/__main__.py", "cli-main-discovery", "unavailable"),
        ):
            with self.subTest(gate=gate):
                path = self.extension / relative
                body = path.read_bytes()
                path.unlink()
                try:
                    code, result = self.child("normal")
                    self.assertEqual(code, 29)
                    self.assertEqual(result, observer.rejected(gate, reason))
                    self.assertFalse(self.audit.exists())
                finally:
                    path.write_bytes(body)

    def test_each_runtime_preflight_guard_exports_only_its_fixed_identifier(self):
        body = self.bootstrap.read_text()
        for setup, gate in (
            ("sys.implementation.name = 'fixture-sensitive'", "runtime-implementation"),
            ("sys.version_info = (3, 15, 0)", "runtime-python-version"),
            ("sys.settrace(lambda *args: None)", "runtime-trace"),
        ):
            with self.subTest(gate=gate):
                self.bootstrap.write_text(
                    body.replace("observer.main()", setup + "\n    observer.main()")
                )
                code, result = self.child("normal")
                self.assertEqual(code, 29)
                self.assertEqual(result, observer.rejected(gate, "unavailable"))
                self.assertFalse(self.audit.exists())
        self.bootstrap.write_text(body)

    def test_runtime_entrypoint_code_mismatch_keeps_cli_result_without_source_hash(
        self,
    ):
        body = self.bootstrap.read_text()
        setup = (
            "import runpy\n"
            "    original = runpy._get_module_details\n"
            "    def changed(*args, **kwargs):\n"
            "        name, spec, code = original(*args, **kwargs)\n"
            "        return name, spec, code.replace(co_firstlineno=code.co_firstlineno + 1)\n"
            "    runpy._get_module_details = changed\n"
            "    observer.main()"
        )
        self.bootstrap.write_text(body.replace("observer.main()", setup))
        code, result = self.child("normal")
        self.assertEqual(code, 0)
        self.assertEqual(result, observer.rejected("cli-main-code", "code-mismatch"))
        self.assertTrue(self.audit.exists())

    def test_runtime_function_code_mismatch_identifies_only_the_fixed_boundary(self):
        body = self.bootstrap.read_text()
        self.bootstrap.write_text(
            body.replace(
                "observer.main()",
                "from azext_devops.dev.common.services import _get_credentials\n"
                "    _get_credentials.__code__ = _get_credentials.__code__.replace(co_firstlineno=_get_credentials.__code__.co_firstlineno + 1)\n"
                "    observer.main()",
            )
        )
        code, result = self.child("normal")
        self.assertEqual(code, 0)
        self.assertEqual(
            result, observer.rejected("credential-resolution-function", "code-mismatch")
        )
        self.assertTrue(self.audit.exists())

    def test_oversized_source_keeps_the_existing_bound_and_exports_no_partial_hash(
        self,
    ):
        path = self.extension / "azext_devops/dev/common/external_tool.py"
        path.write_bytes(b"x" * (64 * 1024 + 1))
        code, result = self.child("normal")
        self.assertEqual(code, 29)
        self.assertEqual(
            result, observer.rejected("extension-external-tool-source", "too-large")
        )
        self.assertFalse(self.audit.exists())

    def test_source_compile_and_import_errors_export_no_exception_text(self):
        path = self.extension / "azext_devops/dev/common/external_tool.py"
        original_source = path.read_bytes()
        invalid_source = b"fixture-sensitive-source invalid syntax\n"
        original_bootstrap = self.bootstrap.read_text()
        path.write_bytes(invalid_source)
        self.bootstrap.write_text(
            original_bootstrap.replace(
                "observer.main()",
                "digest, functions = observer.CONTRACTS['dev.common.external_tool']\n"
                "    observer.CONTRACTS['dev.common.external_tool'] = ("
                + repr(hashlib.sha256(invalid_source).hexdigest())
                + ", functions)\n"
                "    observer.main()",
            )
        )
        code, result = self.child("normal")
        self.assertEqual(code, 29)
        self.assertEqual(
            result,
            observer.rejected("extension-external-tool-source", "compile-unavailable"),
        )
        path.write_bytes(original_source)
        self.bootstrap.write_text(
            original_bootstrap.replace(
                "observer.main()",
                "def missing(*args):\n"
                "        raise ImportError('fixture-sensitive-import https://fixture.invalid/?secret')\n"
                "    observer.importlib.util.find_spec = missing\n"
                "    observer.main()",
            )
        )
        code, result = self.child("normal")
        self.assertEqual(code, 29)
        self.assertEqual(result, observer.rejected("cli-main-discovery", "unavailable"))
        self.assertFalse(self.audit.exists())

    def test_missing_fixed_function_contract_is_not_an_observed_download_failure(self):
        body = self.bootstrap.read_text()
        self.bootstrap.write_text(
            body.replace(
                "observer.main()",
                "observer.CONTRACTS['dev.common.external_tool'][1]['absent_fixture_function'] = 'tool-spawn'\n"
                "    observer.main()",
            )
        )
        code, result = self.child("normal")
        self.assertEqual(code, 29)
        self.assertEqual(
            result, observer.rejected("tool-spawn-function", "function-unavailable")
        )
        self.assertFalse(self.audit.exists())

    def test_parent_gate_rejection_and_original_cli_exit_one_remain_separate(self):
        self.az.write_text("#!/bin/sh\nexit 1\n")
        path = self.extension / "azext_devops/dev/common/external_tool.py"
        changed = path.read_bytes() + b"\n# fixture-sensitive-private-source\n"
        path.write_bytes(changed)
        with (
            patch.object(
                inspector.base_observer.shutil, "which", return_value=str(self.az)
            ),
            patch.object(inspector.base_observer, "launch", side_effect=self.wrapped),
            patch.object(
                inspector, "base_cli_versions", return_value={"status": "unavailable"}
            ),
            patch.dict(os.environ, {"AZURE_EXTENSION_DIR": str(self.extension.parent)}),
            self.assertRaisesRegex(
                inspector.Failure, "^exact-base-universal-download-failed$"
            ) as caught,
        ):
            inspector.command(
                ["az", "artifacts", "universal", "download", "normal"],
                failure_fact="exact-base-universal-download-failed",
            )
        diagnostic = caught.exception.diagnostic
        self.assertEqual(diagnostic["exitCode"], 1)
        self.assertEqual(
            diagnostic["observation"],
            observer.rejected(
                "extension-external-tool-source",
                "hash-mismatch",
                hashlib.sha256(changed).hexdigest(),
            ),
        )
        self.assertFalse(self.audit.exists())
        self.assertNotIn(str(self.root), json.dumps(diagnostic))
        self.assertNotIn("fixture-sensitive", json.dumps(diagnostic))

    def test_late_fork_child_cannot_publish_and_exec_child_cannot_inherit_channel(self):
        code, result = self.child("late-child")
        self.assertEqual(code, 17)
        self.assertEqual(result["status"], "available")
        self.assertEqual(result["innerExitCode"], 0)
        code, result = self.child("fd-probe")
        self.assertEqual(code, 0)
        self.assertEqual(result["innerExitCode"], 0)

    def test_original_timeout_and_cancellation_cleanup_are_authoritative(self):
        for primary in ("timeout", "cancelled"):
            with self.subTest(primary=primary):
                with (
                    patch.object(
                        inspector.base_observer.shutil,
                        "which",
                        return_value=str(self.az),
                    ),
                    patch.object(
                        inspector.base_observer, "launch", side_effect=self.wrapped
                    ),
                    patch.dict(
                        os.environ, {"AZURE_EXTENSION_DIR": str(self.extension.parent)}
                    ),
                ):
                    before_fds = len(list(Path("/proc/self/fd").iterdir()))
                    processes = []
                    popen = subprocess.Popen

                    def capture(*args, **kwargs):
                        process = popen(*args, **kwargs)
                        processes.append(process)
                        return process

                    capture_patch = patch.object(
                        inspector.subprocess, "Popen", side_effect=capture
                    )
                    if primary == "timeout":
                        with (
                            capture_patch,
                            self.assertRaises(subprocess.TimeoutExpired),
                        ):
                            inspector.command(
                                [
                                    "az",
                                    "artifacts",
                                    "universal",
                                    "download",
                                    "timeout",
                                ],
                                timeout=0.2,
                                failure_fact="exact-base-universal-download-failed",
                            )
                    else:
                        original = subprocess.Popen.communicate

                        def cancel(process, *args, **kwargs):
                            raise inspector.Cancelled()

                        with patch.object(popen, "communicate", cancel), capture_patch:
                            with self.assertRaises(inspector.Cancelled):
                                inspector.command(
                                    [
                                        "az",
                                        "artifacts",
                                        "universal",
                                        "download",
                                        "timeout",
                                    ],
                                    failure_fact="exact-base-universal-download-failed",
                                )
                        self.assertIs(subprocess.Popen.communicate, original)
                    self.assertEqual(len(processes), 1)
                    self.assertIsNotNone(processes[0].poll())
                    self.assertEqual(
                        len(list(Path("/proc/self/fd").iterdir())), before_fds
                    )

    def test_parent_real_failure_retains_outer_exit_and_phase_snapshot(self):
        with (
            patch.object(
                inspector.base_observer.shutil, "which", return_value=str(self.az)
            ),
            patch.object(inspector.base_observer, "launch", side_effect=self.wrapped),
            patch.object(
                inspector, "base_cli_versions", return_value={"status": "unavailable"}
            ),
            patch.dict(os.environ, {"AZURE_EXTENSION_DIR": str(self.extension.parent)}),
            self.assertRaises(inspector.Failure) as caught,
        ):
            inspector.command(
                ["az", "artifacts", "universal", "download", "inner-failed"],
                failure_fact="exact-base-universal-download-failed",
            )
        diagnostic = caught.exception.diagnostic
        self.assertEqual(diagnostic["exitCode"], 17)
        self.assertEqual(diagnostic["observation"]["innerExitCode"], 7)
        self.assertNotIn("fixture-sensitive", json.dumps(diagnostic))

    def test_parent_missing_content_length_failure_with_cleanup_stays_primary(self):
        with (
            patch.object(
                inspector.base_observer.shutil, "which", return_value=str(self.az)
            ),
            patch.object(inspector.base_observer, "launch", side_effect=self.wrapped),
            patch.object(
                inspector, "base_cli_versions", return_value={"status": "unavailable"}
            ),
            patch.dict(os.environ, {"AZURE_EXTENSION_DIR": str(self.extension.parent)}),
            self.assertRaisesRegex(
                inspector.Failure, "^exact-base-universal-download-failed$"
            ) as caught,
        ):
            inspector.command(
                ["az", "artifacts", "universal", "download", "length-missing"],
                failure_fact="exact-base-universal-download-failed",
            )
        diagnostic = caught.exception.diagnostic
        self.assertEqual(diagnostic["exitCode"], 17)
        phase = diagnostic["observation"]["phases"]["tool-download-extract"]
        self.assertEqual(phase["exceptionKind"], "KeyError")
        self.assertIsNone(phase["returned"])
        self.assertIs(diagnostic["observation"]["contentLengthValid"], False)
        self.assertNotIn("fixture-sensitive", json.dumps(diagnostic))

    def test_original_child_with_missing_channel_keeps_its_failure(self):
        with (
            patch.object(
                inspector.base_observer.shutil, "which", return_value=str(self.az)
            ),
            patch.object(
                inspector.base_observer,
                "launch",
                return_value=[str(self.az)],
            ),
            patch.object(
                inspector, "base_cli_versions", return_value={"status": "unavailable"}
            ),
            self.assertRaises(inspector.Failure) as caught,
        ):
            inspector.command(
                ["az", "artifacts", "universal", "download"],
                failure_fact="exact-base-universal-download-failed",
            )
        self.assertEqual(caught.exception.diagnostic["exitCode"], 29)
        self.assertEqual(
            caught.exception.diagnostic["observation"], observer.unavailable()
        )

    def test_launcher_contract_keeps_interpreter_path_and_rejects_substitutions(self):
        launcher = self.root / "usr/bin/az"
        launcher.parent.mkdir(parents=True)
        launcher.write_bytes(
            b'#!/usr/bin/env bash\nbin_dir=`cd "$(dirname "$BASH_SOURCE[0]")"; pwd`\n'
            b'AZ_INSTALLER=DEB "$bin_dir"/../../opt/az/bin/python3 -Im azure.cli "$@"\n'
        )
        interpreter = self.root / "opt/az/bin/python3"
        interpreter.parent.mkdir(parents=True)
        interpreter.symlink_to(sys.executable)
        argv = [str(launcher), "artifacts", "universal", "download"]
        self.assertEqual(
            observer.launch(argv, 42),
            [
                str(launcher.parent / "../../opt/az/bin/python3"),
                "-I",
                str(SCRIPT),
                "42",
                *argv,
            ],
        )
        link = launcher.with_name("az-link")
        link.symlink_to(launcher)
        self.assertIsNone(observer.launch([str(link), *argv[1:]], 42))
        launcher.write_bytes(launcher.read_bytes() + b"# fixture-unexpected\n")
        self.assertIsNone(observer.launch(argv, 42))


class ChannelTests(unittest.TestCase):
    def test_instance_fields_never_invokes_custom_dictionary_property(self):
        class Response:
            @property
            def __dict__(self):
                raise AssertionError("fixture-sensitive-property-invoked")

        self.assertEqual(observer.instance_fields(Response()), {})

    def channel(self, body):
        reader, writer = os.pipe2(os.O_NONBLOCK | os.O_CLOEXEC)
        try:
            os.write(writer, body)
            return observer.read_channel(reader)
        finally:
            os.close(writer)
            os.close(reader)

    def test_missing_malformed_oversized_and_typed_redaction(self):
        self.assertEqual(observer.read_channel(None), observer.unavailable())
        self.assertEqual(self.channel(b""), observer.unavailable())
        for body in (b"fixture-sensitive", b"\xff", b"[]", b"x" * (observer.LIMIT + 1)):
            self.assertEqual(
                self.channel(body), observer.unavailable("invalid-channel")
            )
        for key, value in (
            ("innerExitCode", True),
            ("innerExitCode", 256),
            ("releasePresent", "fixture-sensitive"),
            ("serviceCode", "fixture-sensitive-server-code"),
            ("phases", {"fixture-sensitive": {}}),
            (
                "phases",
                {
                    "tool-spawn": {
                        "entered": True,
                        "returned": True,
                        "exceptionKind": "fixture-sensitive",
                        "httpStatus": 200,
                    }
                },
            ),
            (
                "phases",
                {
                    "tool-spawn": {
                        "entered": True,
                        "returned": True,
                        "exceptionKind": None,
                        "httpStatus": True,
                    }
                },
            ),
        ):
            result = observer.unavailable("available")
            if key == "phases":
                result[key].update(value)
            else:
                result[key] = value
            self.assertEqual(
                self.channel(json.dumps(result).encode()),
                observer.unavailable("invalid-channel"),
            )

    def test_bounded_emitter_and_full_or_closed_channel_do_not_replace_failure(self):
        reader, writer = os.pipe2(os.O_NONBLOCK | os.O_CLOEXEC)
        try:
            observer.emit(writer, observer.unavailable())
            body = os.read(reader, observer.LIMIT + 1)
            self.assertLessEqual(len(body), observer.LIMIT)
            self.assertEqual(json.loads(body), observer.unavailable())
            while True:
                try:
                    os.write(writer, b"x" * observer.LIMIT)
                except BlockingIOError:
                    break
            observer.emit(writer, observer.unavailable())
            os.close(reader)
            reader = None
            observer.emit(writer, observer.unavailable())
        finally:
            if reader is not None:
                os.close(reader)
            os.close(writer)

    def test_gate_catalog_hash_validation_redaction_and_channel_bound(self):
        digest = hashlib.sha256(b"fixture source").hexdigest()
        valid = observer.rejected("extension-updater-source", "hash-mismatch", digest)
        self.assertEqual(self.channel(json.dumps(valid).encode()), valid)
        self.assertLessEqual(len(json.dumps(valid).encode()), observer.LIMIT)
        for key, item in (
            ("id", "fixture-sensitive-path"),
            ("reason", "fixture-sensitive-server-string"),
            ("reason", "binding-mismatch"),
            ("observedSha256", "https://fixture.invalid/?secret"),
            ("observedSha256", "F" * 64),
            ("observedSha256", None),
        ):
            with self.subTest(key=key, item=item):
                value = observer.rejected(
                    "extension-updater-source", "hash-mismatch", digest
                )
                value["gate"][key] = item
                result = self.channel(json.dumps(value).encode())
                self.assertEqual(result, observer.unavailable("invalid-channel"))
                self.assertNotIn("fixture-sensitive", json.dumps(result))
        value = observer.unavailable("available")
        value["gate"] = valid["gate"]
        self.assertEqual(
            observer.validate(value), observer.unavailable("invalid-channel")
        )


if __name__ == "__main__":
    unittest.main()
