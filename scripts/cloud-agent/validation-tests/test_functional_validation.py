import argparse
import copy
import importlib.util
import io
import json
import os
import shlex
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
CANDIDATE = "18dd6cd99d50a4268afd936acb1ee4d2b64881ea"


class EvidenceTests(unittest.TestCase):
    def setUp(self):
        cases = {
            "positive": {"markers": ["functional", "helpers", "positive"]},
            "negative": {"markers": ["functional", "helpers", "negative"]},
        }
        self.manifest = {"crate": {"submodules": {"module": {"test_cases": cases}}}}

    def test_default_and_explicit_candidate_inventory(self):
        self.assertEqual(
            validation.SOURCE_TEST_COUNTS,
            {
                "985a0a70b88ea5576805c6fccae6a6ff4d8800a4": 51,
                CANDIDATE: 70,
            },
        )
        for revision, count in validation.SOURCE_TEST_COUNTS.items():
            with self.subTest(
                revision=revision
            ), tempfile.TemporaryDirectory() as directory:
                options = (
                    []
                    if revision == validation.SOURCE
                    else ["--expected-source-revision", revision]
                )
                args = validation.parse_args([directory, directory, *options])
                harness = validation.Validation(
                    args.source, args.reports, args.expected_source_revision
                )
                self.assertEqual(harness.expected_source_revision, revision)
                self.assertEqual(harness.source_test_count, count)
                harness.validate_source_tests(f"Ran {count} tests\nOK\n")
                harness.validate_source_tests(
                    "test_provisioned_lock_reopens_across_uids_with_protected_regular "
                    f"... skipped 'requires root'\nRan {count} tests\nOK (skipped=1)"
                )
                for wrong in (0, count - 1, count + 1, 51 if count == 70 else 70):
                    with self.assertRaisesRegex(RuntimeError, "test count changed"):
                        harness.validate_source_tests(f"Ran {wrong} tests\nOK\n")
                with self.assertRaisesRegex(
                    RuntimeError, "Unexpected source test skip"
                ):
                    harness.validate_source_tests(
                        f"test_other ... skipped 'unexpected'\nRan {count} tests\nOK"
                    )

    def test_unqualified_candidates_and_missing_values_fail_before_work(self):
        for revision in (
            "",
            "main",
            "HEAD",
            "18dd6cd",
            CANDIDATE.upper(),
            CANDIDATE + "^",
            "a" * 40,
            "g" * 40,
        ):
            with self.subTest(
                revision=revision
            ), tempfile.TemporaryDirectory() as directory:
                reports = Path(directory) / "reports"
                with patch("sys.stderr", new=io.StringIO()), self.assertRaises(
                    SystemExit
                ) as error:
                    validation.parse_args(
                        [
                            directory,
                            str(reports),
                            "--expected-source-revision",
                            revision,
                        ]
                    )
                self.assertEqual(error.exception.code, 2)
                with self.assertRaises(argparse.ArgumentTypeError):
                    validation.Validation(directory, reports, revision)
                self.assertFalse(reports.exists())
        for argv in (
            ["source", "reports", "--expected-source-revision"],
            ["source"],
            ["source", "reports", "--source-test-count", "1"],
        ):
            with self.subTest(argv=argv), patch(
                "sys.stderr", new=io.StringIO()
            ), self.assertRaises(SystemExit):
                validation.parse_args(argv)

    def test_candidate_identity_labels_and_checkout_mismatch_rejection(self):
        for revision in validation.SOURCE_TEST_COUNTS:
            for matches in (True, False):
                with self.subTest(
                    revision=revision, matches=matches
                ), tempfile.TemporaryDirectory() as directory:
                    source = Path(directory)
                    descriptor = source / "scripts/cloud-agent/functional-artifact.json"
                    descriptor.parent.mkdir(parents=True)
                    descriptor.write_text("{}")
                    harness = validation.Validation(
                        source, source / "reports", revision
                    )

                    def run(name, args):
                        if name == "source-identity":
                            self.assertEqual(args, ["git", "rev-parse", "HEAD"])
                            return revision if matches else "a" * 40
                        raise RuntimeError("stop before preparation")

                    with patch.dict(
                        os.environ,
                        GITHUB_ACTIONS="true",
                        EXCLUSIVE_DISPOSABLE_1ES="true",
                        GITHUB_SHA="harness",
                    ), patch.object(harness, "run", side_effect=run) as command:
                        expected = (
                            "stop before preparation"
                            if matches
                            else "Wrong pinned source"
                        )
                        with self.assertRaisesRegex(RuntimeError, expected):
                            harness.execute()
                    identity = json.loads(
                        (harness.reports / "identity.json").read_text()
                    )
                    self.assertEqual(identity["candidate"], revision)
                    self.assertEqual(command.call_count, 2 if matches else 1)

    def test_main_forwards_default_and_explicit_candidate(self):
        for options, revision in (
            ([], validation.SOURCE),
            (["--expected-source-revision", CANDIDATE], CANDIDATE),
        ):
            with self.subTest(revision=revision), patch.object(
                sys, "argv", ["functional-validation.py", "source", "reports", *options]
            ), patch.object(validation, "Validation") as factory, patch.object(
                validation.signal, "signal"
            ):
                self.assertEqual(validation.main(), 0)
                factory.assert_called_once_with("source", "reports", revision)

    def test_missing_checkout_fails_before_commands(self):
        with tempfile.TemporaryDirectory() as directory:
            missing = Path(directory) / "missing"
            harness = validation.Validation(
                missing, Path(directory) / "reports", CANDIDATE
            )
            with patch.dict(
                os.environ,
                GITHUB_ACTIONS="true",
                EXCLUSIVE_DISPOSABLE_1ES="true",
                GITHUB_SHA="harness",
            ), patch.object(harness, "run") as command:
                with self.assertRaises(FileNotFoundError):
                    harness.execute()
            command.assert_not_called()
            self.assertFalse(missing.exists())

    def test_explicit_candidate_binary_evidence_labels(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory)
            (source / "target").mkdir()
            harness = validation.Validation(source, source / "reports", CANDIDATE)
            harness.baseline_inventory = {"domains": {}, "networks": {}, "pools": {}}
            harness.baseline_lock = "unchanged"

            def failing_run(name, args, **kwargs):
                if name.endswith("-namespace-clean"):
                    return ""
                Path(kwargs["env"]["VALIDATION_BINARIES"]).write_text(
                    '[{"sha256":"observed"}]'
                )
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
            evidence = json.loads((harness.reports / "smoke-binaries.json").read_text())
            self.assertEqual(evidence["candidate"], CANDIDATE)
            self.assertEqual(evidence["binaries"], [{"sha256": "observed"}])

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

    def test_go_excludes_only_exact_baseline_test_and_records_limitation(self):
        with tempfile.TemporaryDirectory() as directory:
            harness = validation.Validation(directory, Path(directory) / "reports")
            with patch.object(harness, "run") as run:
                harness.validate_go()
            name, args = run.call_args.args
            self.assertEqual(name, "go-filtered")
            self.assertEqual(args[:2], ["bash", "-ec"])
            self.assertEqual(
                [shlex.split(step) for step in args[2].split(";")],
                [
                    ["cd", "tools"],
                    ["go", "generate", "pkg/rcp/tlscerts/certs.go"],
                    ["go", "generate", "pkg/tridentgrpc/grpc.go"],
                    [
                        "go",
                        "test",
                        "-mod=readonly",
                        "-skip",
                        "^TestUefiFallbackScriptMatchesLegacy$",
                        "./...",
                    ],
                    [
                        "go",
                        "test",
                        "-mod=readonly",
                        "-race",
                        "./pkg/virtdeploy/...",
                    ],
                ],
            )
            excluded = json.loads((harness.reports / "go-exclusions.json").read_text())
            self.assertEqual(excluded["test"], "TestUefiFallbackScriptMatchesLegacy")
            self.assertEqual(excluded["package"], "tridenttools/storm/e2e/scenario")
            self.assertEqual(excluded["status"], "excluded/not validated")
            self.assertFalse(excluded["full_go_suite_validated"])
            self.assertIn("45248620733", excluded["evidence"])
            self.assertIn("full Go suite not validated", validation.GO_BASELINE_LIMIT)
            self.assertIn("go-filtered", validation.PLANNED)
            self.assertNotIn("go", validation.PLANNED)

    def test_filtered_go_keeps_other_failures_fatal(self):
        with tempfile.TemporaryDirectory() as directory:
            harness = validation.Validation(directory, Path(directory) / "reports")
            with patch.object(
                harness, "run", side_effect=RuntimeError("unrelated Go failure")
            ):
                with self.assertRaisesRegex(RuntimeError, "unrelated Go failure"):
                    harness.validate_go()
            self.assertTrue((harness.reports / "go-exclusions.json").exists())

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
            harness = validation.Validation(source, source / "reports", CANDIDATE)
            harness.results = {
                name: {"functional_base_image": str(image)}
                for name in ("smoke", "full")
            }
            harness.cache_provenance("smoke")
            saved = json.loads((harness.reports / "smoke-provenance.json").read_text())
            self.assertEqual(saved, provenance)
            self.assertEqual(saved["build"]["sourceVersion"], "a" * 40)
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
