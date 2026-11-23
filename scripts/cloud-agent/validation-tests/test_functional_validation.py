import copy
import importlib.util
import json
import os
import sys
import tempfile
import unittest
import xml.etree.ElementTree as ET
from pathlib import Path
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location(
    "functional_validation",
    Path(__file__).resolve().parents[1] / "functional-validation.py",
)
validation = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(validation)


class EvidenceTests(unittest.TestCase):
    def setUp(self):
        cases = {
            "positive": {"markers": ["functional", "helpers", "positive"]},
            "negative": {"markers": ["functional", "helpers", "negative"]},
        }
        self.manifest = {"crate": {"submodules": {"module": {"test_cases": cases}}}}

    def junit(self, mode="full", prefix="ft.json."):
        root = ET.Element("testsuites")
        for name in ("test_create_vm", "test_deployment"):
            ET.SubElement(root, "testcase", name=name, classname="test_setup")
        for name in ("positive",) if mode == "smoke" else ("positive", "negative"):
            case = ET.SubElement(
                root, "testcase", name=name, classname=prefix + "crate.module"
            )
            ET.SubElement(case, "system-out").text = (
                "test result: ok. 1 passed; 0 failed;"
            )
        return root

    def test_both_junit_classpath_forms_and_manifest_selection(self):
        for mode, count in (("smoke", 1), ("full", 2)):
            for prefix in ("ft.json.", "tests.functional_tests.ft.json."):
                with self.subTest(mode=mode, prefix=prefix):
                    result = validation.verify_junit(
                        self.junit(mode, prefix), self.manifest, mode
                    )
                    self.assertEqual(result["rust_passed"], count)

    def test_missing_duplicate_and_unknown_rust_cases_fail(self):
        for kind in ("missing", "duplicate", "unknown"):
            root = self.junit()
            if kind == "missing":
                root.remove(root[-1])
            elif kind == "duplicate":
                root.append(copy.deepcopy(root[-1]))
            else:
                root[-1].set("name", "unexpected")
            with self.subTest(kind=kind), self.assertRaises(RuntimeError):
                validation.verify_junit(root, self.manifest, "full")

    def test_failures_errors_and_skips_fail(self):
        for tag in ("failure", "error", "skipped"):
            root = self.junit()
            ET.SubElement(root[-1], tag)
            with self.subTest(tag=tag), self.assertRaises(RuntimeError):
                validation.verify_junit(root, self.manifest, "full")

    def test_zero_rust_execution_is_not_a_pass(self):
        root = self.junit()
        root[-1].find("system-out").text = "test result: ok. 0 passed; 0 failed;"
        with self.assertRaisesRegex(RuntimeError, "one Rust test"):
            validation.verify_junit(root, self.manifest, "full")

    def test_vm_and_ssh_are_required(self):
        for name in ("test_create_vm", "test_deployment"):
            root = self.junit()
            root.remove(next(c for c in root if c.get("name") == name))
            with self.subTest(name=name), self.assertRaisesRegex(
                RuntimeError, "Real VM"
            ):
                validation.verify_junit(root, self.manifest, "full")

    def test_unexpected_manifest_skips_fail(self):
        self.manifest["crate"]["submodules"]["module"]["test_cases"]["positive"][
            "skip"
        ] = "reason"
        with self.assertRaisesRegex(RuntimeError, "skip/xfail"):
            validation.verify_junit(self.junit(), self.manifest, "full")

    def test_redaction_preserves_valid_xml(self):
        with tempfile.TemporaryDirectory() as directory:
            harness = validation.Validation(directory, Path(directory) / "reports")
            report = Path(directory) / "input.xml"
            root = self.junit()
            root[-1].find("system-out").text = (
                "https://example.invalid/?sig=not-a-secret\n"
                "Authorization: placeholder"
            )
            ET.ElementTree(root).write(report)
            harness.capture_junit("redacted", report, None)
            saved = (harness.reports / "redacted.xml").read_text()
            ET.fromstring(saved)
            self.assertNotIn("not-a-secret", saved)
            self.assertNotIn("Authorization:", saved)
            with self.assertRaisesRegex(RuntimeError, "no fresh JUnit"):
                harness.capture_junit("stale", report, report.stat().st_mtime_ns)

    def test_failed_run_still_captures_fresh_junit_and_inventory(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory)
            (source / "target").mkdir()
            harness = validation.Validation(source, source / "reports")
            harness.baseline_inventory = {"domains": {}, "networks": {}, "pools": {}}
            harness.baseline_lock = "unchanged"

            def failing_run(*args, **kwargs):
                if args[0].endswith("-namespace-clean"):
                    return ""
                ET.ElementTree(self.junit()).write(
                    source / "target/trident_functional_tests.xml"
                )
                raise RuntimeError("primary failure")

            with patch.dict(
                os.environ, GITHUB_RUN_ID="1", GITHUB_RUN_ATTEMPT="1"
            ), patch.object(harness, "run", side_effect=failing_run), patch.object(
                harness, "inventory", return_value=harness.baseline_inventory
            ), patch.object(
                harness, "lock_identity", return_value="unchanged"
            ):
                with self.assertRaisesRegex(RuntimeError, "primary failure"):
                    harness.functional("smoke", "smoke")
            self.assertTrue((harness.reports / "smoke.xml").exists())
            self.assertTrue((harness.reports / "smoke-inventory.json").exists())

    def test_command_exit_and_bounded_log_are_recorded(self):
        with tempfile.TemporaryDirectory() as directory:
            harness = validation.Validation(directory, Path(directory) / "reports")
            with patch.object(validation, "MAX_LOG", 64):
                harness.run(
                    "expected-failure",
                    [sys.executable, "-c", "print('x' * 1000); raise SystemExit(2)"],
                    expected=2,
                )
            result = json.loads((harness.reports / "results.json").read_text())
            self.assertEqual(result["expected-failure"]["exit"], 2)
            self.assertTrue(result["expected-failure"]["log_bounded"])
            with self.assertRaises(RuntimeError):
                harness.run("wrong-exit", [sys.executable, "-c", "raise SystemExit(1)"])

    def test_plugin_hashes_candidate_binary_without_modifying_it(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory)
            binary = source / "crate-12345"
            binary.write_bytes(b"candidate binary placeholder")
            build_output = source / "build.json"
            build_output.write_text(
                json.dumps({"target": {"kind": ["lib"]}, "executable": str(binary)})
            )
            session = type(
                "Session",
                (),
                {
                    "config": type(
                        "Config",
                        (),
                        {"getoption": lambda self, option: str(build_output)},
                    )()
                },
            )()
            plugin = {}
            exec(validation.PLUGIN, plugin)
            report = source / "binaries.json"
            with patch.dict(os.environ, VALIDATION_BINARIES=str(report)):
                plugin["pytest_sessionstart"](session)
            observed = json.loads(report.read_text())
            self.assertEqual(observed[0]["remote"], "tests/crate")
            self.assertEqual(observed[0]["sha256"], validation.digest(binary))
            self.assertEqual(binary.read_bytes(), b"candidate binary placeholder")

    def test_json_redaction_stays_parseable(self):
        with tempfile.TemporaryDirectory() as directory:
            harness = validation.Validation(directory, Path(directory) / "reports")
            harness.save("redacted.json", {"stderr": "Authorization: placeholder"})
            value = json.loads((harness.reports / "redacted.json").read_text())
            self.assertEqual(value["stderr"], "[REDACTED_CREDENTIAL_LINE]")

    def test_immutable_cache_build_pin_and_reuse(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory)
            image = source / "image.qcow2"
            image.write_bytes(b"image placeholder")
            provenance_path = source / "provenance.json"
            provenance = {
                "build": {
                    "id": 123,
                    "definitionId": 5067,
                    "sourceBranch": "refs/heads/main",
                    "status": "completed",
                    "result": "succeeded",
                    "sourceVersion": "a" * 40,
                },
                "image": {
                    "sha256": validation.digest(image),
                    "size": image.stat().st_size,
                },
                "cacheHit": False,
            }
            provenance_path.write_text(json.dumps(provenance))
            harness = validation.Validation(source, source / "reports")
            harness.results = {
                name: {"functional_base_image": str(image)}
                for name in ("smoke", "full")
            }
            harness.cache_provenance("smoke")
            self.assertEqual(harness.env["TRIDENT_FUNCTIONAL_BUILD_ID"], "123")
            with self.assertRaisesRegex(RuntimeError, "reuse"):
                harness.cache_provenance("full")
            provenance["cacheHit"] = True
            provenance_path.write_text(json.dumps(provenance))
            harness.cache_provenance("full")
            provenance["build"]["sourceVersion"] = "b" * 40
            provenance_path.write_text(json.dumps(provenance))
            with self.assertRaisesRegex(RuntimeError, "reuse"):
                harness.cache_provenance("full")
            provenance["build"]["sourceVersion"] = "a" * 40
            provenance_path.write_text(json.dumps(provenance))
            image.write_bytes(b"unexpected change")
            self.assertEqual(image.stat().st_size, provenance["image"]["size"])
            with self.assertRaisesRegex(RuntimeError, "reuse"):
                harness.cache_provenance("full")
            image.write_bytes(b"different length")
            with self.assertRaisesRegex(RuntimeError, "Invalid immutable"):
                harness.cache_provenance("full")


if __name__ == "__main__":
    unittest.main()
