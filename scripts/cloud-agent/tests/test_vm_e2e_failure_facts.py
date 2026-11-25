import importlib.util
import json
import os
import subprocess
import tempfile
import unittest

from pathlib import Path
from urllib.error import HTTPError, URLError

SCRIPT = Path(__file__).resolve().parents[1] / "vm_e2e_failure_facts.py"
SPEC = importlib.util.spec_from_file_location("failure_facts", SCRIPT)
facts = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(facts)


class FailureFactsTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)

    def test_only_typed_http_and_process_status_not_error_text_are_retained(self):
        error = RuntimeError("fixture-private HTTP 503 auth-url")
        value = facts.fact("acquire-build", error)
        self.assertIsNone(value["exceptionClass"])
        self.assertIsNone(value["httpStatus"])
        error.__cause__ = HTTPError("fixture-private-url", 403, "private", None, None)
        self.addCleanup(error.__cause__.close)
        value = facts.fact("acquire-build", error)
        self.assertEqual(value["exceptionClass"], "HTTPError")
        self.assertEqual(value["httpStatus"], 403)
        self.assertNotIn("fixture-private", json.dumps(value))
        value = facts.fact(
            "acquire-token",
            subprocess.CalledProcessError(37, ["fixture-private HTTP 503"]),
        )
        self.assertEqual(value["exitCode"], 37)
        self.assertIsNone(value["httpStatus"])
        self.assertEqual(value["exceptionClass"], "CalledProcessError")
        self.assertEqual(
            facts.fact("acquire-build", URLError("fixture-private"))["exceptionClass"],
            "URLError",
        )
        error.__cause__ = error
        self.assertIsNone(facts.fact("acquire-build", error)["httpStatus"])

    def test_closed_schema_limits_refuse_untrusted_types_names_and_extra_fields(self):
        original = facts.fact("identity-umi-login")
        for field, value in (
            ("operation", "fixture-private"),
            ("operation", []),
            ("exitCode", True),
            ("exitCode", 256),
            ("exceptionClass", "fixture-private"),
            ("exceptionClass", []),
            ("httpStatus", True),
            ("httpStatus", 999),
            ("httpStatus", "503"),
            ("url", "fixture-private"),
        ):
            with self.subTest(field=field, value=value):
                row = {**original, field: value}
                with self.assertRaises(ValueError):
                    facts.validate(row)

    def test_first_failure_latches_and_observer_failure_has_publication_boundary(self):
        events = []
        trace = facts.Trace(lambda state, value: events.append((state, value)))
        trace.start("identity-extension")
        trace.fail(exit_code=37)
        trace.start("identity-umi-login")
        trace.fail(ValueError("fixture-private"))
        self.assertEqual(trace.failure, facts.fact("identity-extension", exit_code=37))
        self.assertNotIn("fixture-private", json.dumps(events))

        def broken(state, value):
            raise OSError("fixture-private")

        trace = facts.Trace(broken)
        with self.assertRaises(OSError):
            trace.start("identity-umi-login")
        self.assertEqual(trace.operation, "checkpoint-publication")

    def test_private_handoff_retains_primary_and_distinct_cleanup_without_raw_values(
        self,
    ):
        path = self.root / "facts.json"
        with facts.private_handoff(path) as emit:
            trace = facts.Trace(emit)
            trace.start("artifact-download-go-tools")
            self.assertEqual(facts.load_handoff(path)["state"], "started")
            trace.fail(URLError("fixture-private"))
            trace.start("acquire-staging-cleanup")
            trace.fail(PermissionError("fixture-private"), secondary=True)
            row = facts.load_handoff(path)
        self.assertEqual(row["state"], "failed")
        self.assertEqual(row["fact"]["operation"], "artifact-download-go-tools")
        self.assertEqual(row["fact"]["exceptionClass"], "URLError")
        self.assertEqual(row["cleanupFailure"]["operation"], "acquire-staging-cleanup")
        self.assertNotIn("fixture-private", path.read_text())
        self.assertLessEqual(path.stat().st_size, facts.LIMIT)
        with self.assertRaises(FileExistsError):
            with facts.private_handoff(path):
                self.fail("existing handoff replaced")

    def test_handoff_refuses_public_permissions_links_oversize_and_wrong_operations(
        self,
    ):
        path = self.root / "facts.json"
        with facts.private_handoff(path) as emit:
            emit("started", facts.fact("acquire-token"))
        raw = path.read_bytes()
        for kind in ("mode", "hardlink", "symlink", "oversize", "operation", "fields"):
            with self.subTest(kind=kind):
                path.chmod(0o600)
                path.write_bytes(raw)
                if kind == "mode":
                    path.chmod(0o644)
                elif kind == "hardlink":
                    alias = self.root / "alias"
                    os.link(path, alias)
                elif kind == "symlink":
                    path.unlink()
                    path.symlink_to(self.root / "missing")
                elif kind == "oversize":
                    path.write_bytes(b"x" * (facts.LIMIT + 1))
                else:
                    value = json.loads(raw)
                    if kind == "operation":
                        value["fact"]["operation"] = "identity-umi-login"
                    else:
                        value["url"] = "fixture-private"
                    path.write_text(json.dumps(value))
                with self.assertRaises((OSError, ValueError)):
                    facts.load_handoff(path)
                if kind == "symlink":
                    path.unlink()
                    path.write_bytes(raw)
                if kind == "hardlink":
                    alias.unlink()
        with self.assertRaises(ValueError):
            value = json.loads(raw)
            value["fact"]["exitCode"] = 23
            facts.read_handoff(json.dumps(value).encode())

    def test_cleanup_context_captures_error_before_unwinding(self):
        seen = []
        error = ValueError("fixture-private")
        with self.assertRaises(ValueError):
            with facts.retaining(
                lambda e: seen.append(e),
                lambda op: seen.append(op),
                "acquire-staging-cleanup",
            ):
                raise error
        self.assertEqual(seen, [error, "acquire-staging-cleanup"])
