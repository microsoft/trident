import argparse
import hashlib
import importlib.util
import io
import json
import os
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest
import zipfile

from contextlib import nullcontext
from pathlib import Path
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parents[1] / "prepare_vm_e2e_artifacts.py"
SPEC = importlib.util.spec_from_file_location("prepare_vm", SCRIPT)
resolver = importlib.util.module_from_spec(SPEC)
with patch.object(sys, "path", [str(SCRIPT.parent), *sys.path]):
    SPEC.loader.exec_module(resolver)
runner = resolver.reader()
REVISION = "a" * 40


class PrepareArtifactsTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.args = argparse.Namespace(
            build_id=123,
            expected_definition_id=5073,
            expected_revision=REVISION,
            expected_ref="refs/heads/candidate",
            evidence=self.root / "evidence.json",
            evidence_sha256="",
            output=self.root / "output",
            include_uki_certificate=False,
            timeout_seconds=30,
        )
        self.expected = resolver.producer(self.args)
        self.build = {
            "id": 123,
            "definition": {"id": 5073},
            "project": {"id": resolver.PROJECT_ID},
            "status": "completed",
            "result": "succeeded",
            "sourceBranch": self.args.expected_ref,
            "sourceVersion": REVISION,
        }
        self.records = []
        self.log_metadata = []
        logs = {}
        self.citations = {}
        for index, (artifact, stage) in enumerate(resolver.STAGES.items(), 1):
            self.records.extend(
                [
                    {
                        "id": stage,
                        "identifier": stage,
                        "type": "Stage",
                        "state": "completed",
                        "result": "succeeded",
                    },
                    {
                        "id": f"task-{index}",
                        "parentId": stage,
                        "type": "Task",
                        "state": "completed",
                        "result": "succeeded",
                        "log": {"id": index},
                    },
                ]
            )
            logs[str(index)] = {
                "sha256": hashlib.sha256(f"fixture-log-{index}".encode()).hexdigest(),
                "lineCount": 1,
            }
            self.log_metadata.append({"id": index, "lineCount": 1})
            self.citations[artifact] = {"logId": index, "startLine": 1, "endLine": 1}
        self.packages = [
            {"name": name, "nevra": name + "-fixture.azl3.x86_64", "sha256": char * 64}
            for name, char in (("trident", "b"), ("trident-service", "c"))
        ]
        self.packet = {
            "schemaVersion": 1,
            "producer": self.expected,
            "review": {
                "reviewer": "fixture-reviewer",
                "reference": "fixture-review-only",
            },
            "logs": logs,
            "rpms": {
                "checkoutRevision": REVISION,
                "packages": self.packages,
                "evidence": self.citations["trident-binaries"],
            },
            "artifacts": {},
        }
        self.contents = {}
        self.listing = []
        for index, name in enumerate(resolver.STAGES, 1):
            if name == "trident-binaries":
                continue
            filenames = (
                resolver.TOOLS
                if name == "go-tools"
                else (
                    [name + ".iso"]
                    if name in resolver.INSTALLERS
                    else [f"{name}_{i}.cosi" for i in range(2)]
                )
            )
            citation = self.citations[name]
            artifact = {
                "checkoutRevision": REVISION,
                "checkoutEvidence": citation,
                "rpmBuildId": 123,
                "rpmArtifact": "trident-binaries",
                "rpmEvidence": citation,
                "files": {},
            }
            for kind in ("baseVhdx", "mic"):
                artifact[kind] = {
                    "identity": f"fixture-{kind}-1.2.3",
                    "sha256": "d" * 64,
                    "evidence": citation,
                }
            self.contents[name] = {}
            for filename in filenames:
                # Optional native integration uses the same real Storm helper as ADO.
                data = (
                    Path(os.environ["TRIDENT_TEST_STORM"]).read_bytes()
                    if filename == "storm-trident"
                    and os.environ.get("TRIDENT_TEST_STORM")
                    else f"fixture-only, never boot: {filename}".encode()
                )
                self.contents[name][filename] = data
                artifact["files"][filename] = {
                    "size": len(data),
                    "sha256": hashlib.sha256(data).hexdigest(),
                    "installedPackages": (
                        self.packages[:1]
                        if name in resolver.INSTALLERS
                        else self.packages[1:]
                    ),
                    "installEvidence": citation,
                }
                if name == "go-tools":
                    artifact["files"][filename].pop("installedPackages")
                    artifact["files"][filename].pop("installEvidence")
            if name == "go-tools":
                for field in (
                    "rpmBuildId",
                    "rpmArtifact",
                    "rpmEvidence",
                    "baseVhdx",
                    "mic",
                ):
                    artifact.pop(field)
            self.packet["artifacts"][name] = artifact
            self.listing.append(
                {
                    "id": index,
                    "name": name,
                    "resource": {
                        "type": "PipelineArtifact",
                        "properties": {"artifactsize": "1000"},
                        "downloadUrl": f"https://fixture.artifacts.visualstudio.com/{name}",
                    },
                }
            )
        self.downloads = []
        self.commands = []
        self.extra_entry = None
        self.pin()

    def pin(self):
        self.args.evidence.write_text(json.dumps(self.packet), encoding="utf-8")
        self.args.evidence_sha256 = resolver.ado.hash_file(self.args.evidence)

    def request(self, url, token):
        self.assertEqual(token, "fixture-token")
        self.assertNotIn("queryOrder", url)
        if "/timeline?" in url:
            return {"records": self.records}
        if "/artifacts?" in url:
            return {"value": self.listing}
        if "/logs?" in url:
            return {"value": self.log_metadata}
        self.assertTrue(url.endswith("/build/builds/123?api-version=7.1"))
        return self.build

    def download(self, url, token, destination):
        name = url.rsplit("/", 1)[-1]
        self.downloads.append(name)
        with zipfile.ZipFile(destination, "w") as archive:
            for filename, data in self.contents[name].items():
                archive.writestr(f"{name}/{filename}", data)
            if self.extra_entry:
                archive.writestr(self.extra_entry, b"unsafe")

    def preparation(self, argv, **kwargs):
        self.commands.append(argv)
        self.assertEqual(argv[1:5], ["helper", "prepare-images", "-a", "--"])
        self.assertEqual(argv[8:11], list(resolver.IMAGES))
        self.assertEqual(argv[-2:], ["-v", "4"])
        self.assertTrue(kwargs["check"])
        self.assertEqual(kwargs["timeout"], 30)
        target = Path(argv[11])
        target.mkdir(parents=True)
        for source, (name, stem) in zip(argv[5:8], resolver.IMAGES.items()):
            for i in range(4):
                path = target / (stem + (f"_v{i+1}" if i else "") + ".cosi")
                shutil.copyfile(Path(source) / f"{name}_{i % 2}.cosi", path)
        return subprocess.CompletedProcess(argv, 0)

    def prepare(self, native=False):
        with (
            patch.object(resolver.ado, "request_json", side_effect=self.request),
            patch.object(resolver.ado, "download_file", side_effect=self.download),
            (
                nullcontext()
                if native
                else patch.object(
                    resolver.subprocess, "run", side_effect=self.preparation
                )
            ),
        ):
            return resolver.prepare(self.args, "fixture-token")

    def assert_rejected(self, message):
        with self.assertRaisesRegex((resolver.ado.ArtifactError, KeyError), message):
            self.prepare()
        self.assertFalse((self.args.output / "bundle/provenance.json").exists())

    def test_handoff_round_trip_through_unchanged_c_reader_and_execution_preflight(
        self,
    ):
        self.records[0]["issues"] = [{"message": "fixture-private-timeline-prose"}]
        manifest_path, pin = self.prepare()
        bundle = manifest_path.parent
        manifest = runner.verify_bundle(bundle, manifest_path, pin, REVISION)
        self.assertEqual(manifest["producer"], self.expected)
        self.assertEqual(manifest["lineage"]["packet"], self.packet)
        self.assertNotIn("fixture-private", manifest_path.read_text())
        self.assertNotIn("downloadUrl", manifest_path.read_text())
        self.assertEqual(len(self.downloads), 6)
        self.assertEqual(len(set(self.downloads)), 6)
        self.assertEqual(len(manifest["files"]), 19)
        self.assertFalse((bundle / "artifacts/usrverity/ca_cert.pem").exists())
        self.assertEqual(len(self.commands), 1)
        for name, stem in resolver.IMAGES.items():
            for i in range(4):
                filename = stem + (f"_v{i+1}" if i else "") + ".cosi"
                row = manifest["files"][f"artifacts/test-image/{filename}"]
                self.assertEqual(row["artifact"], name)
                self.assertEqual(row["sourceFile"], f"{name}_{i % 2}.cosi")
        matrix = {
            "base_vm-host": {
                "SCENARIO": "base_vm-host",
                "HARDWARE": "vm",
                "RUNTIME": "host",
                "TEST_RING": "ci",
                "tridentConfigurationName": "base",
                "tridentDeploymentEnv": "virtualMachine",
                "tridentRuntimeEnv": "host",
                "installerISOName": "trident-installer",
            },
        }

        def command(argv, cwd, log, timeout, grace):
            if argv[1] == "script":
                log.write_text(runner.MATRIX_PREFIX + json.dumps(matrix))
            else:
                Path(argv[argv.index("-j") + 1]).write_text(
                    '<testsuite><testcase name="fixture" status="PASS"/></testsuite>'
                )
            return 0

        args = argparse.Namespace(
            bundle=bundle,
            manifest=manifest_path,
            manifest_sha256=pin,
            expected_revision=REVISION,
            output=self.root / "execution",
            stage_type="pr-e2e",
            scenario=None,
            timeout_seconds=1,
            cleanup_grace_seconds=1,
        )
        with (
            patch.object(runner, "host_lock", return_value=nullcontext()),
            patch.object(runner, "check_host"),
            patch.object(runner, "command", side_effect=command),
        ):
            self.assertEqual(runner.execute(args), 0)

    @unittest.skipUnless(
        os.environ.get("TRIDENT_TEST_STORM"), "native Storm build not supplied"
    )
    def test_native_existing_storm_preparation_and_c_reader(self):
        path, pin = self.prepare(native=True)
        runner.verify_bundle(path.parent, path, pin, REVISION)
        images = path.parent / "artifacts/test-image"
        self.assertEqual(
            (images / "regular.cosi").stat().st_ino,
            (images / "regular_v3.cosi").stat().st_ino,
        )
        self.assertNotEqual(
            (images / "regular.cosi").stat().st_ino,
            (images / "regular_v2.cosi").stat().st_ino,
        )

    def test_wrong_source_definition_ref_project_and_failed_build(self):
        original = json.dumps(self.build)
        for mutation in (
            {"sourceVersion": "e" * 40},
            {"definition": {"id": 5067}},
            {"sourceBranch": "refs/heads/main"},
            {"project": {"id": "other"}},
            {"result": "failed"},
            {"status": "inProgress"},
            {"id": 124},
        ):
            with self.subTest(mutation=mutation):
                self.build = {**json.loads(original), **mutation}
                self.assert_rejected("mismatch")
        self.assertEqual(self.downloads, [])

    def test_missing_failed_skipped_or_ambiguous_stage(self):
        original = json.dumps(self.records)
        for result in ("failed", "skipped", "succeededWithIssues"):
            self.records[0]["result"] = result
            self.assert_rejected("did not succeed")
        self.records = json.loads(original)[1:]
        self.assert_rejected("Missing or ambiguous")
        self.records = json.loads(original) + [json.loads(original)[0]]
        self.assert_rejected("Missing or ambiguous")

    def test_unproven_lineage_and_unresolved_inputs_fail_before_download(self):
        original = json.dumps(self.packet)
        name = "trident-testimage"
        for mutate in (
            lambda a: a.update(checkoutRevision="e" * 40),
            lambda a: a.update(rpmBuildId=122),
            lambda a: a.update(rpmArtifact="old-rpms"),
            lambda a: a["files"][f"{name}_1.cosi"].update(installedPackages=[]),
            lambda a: a["files"].pop(f"{name}_1.cosi"),
            lambda a: a["baseVhdx"].update(identity="latest-*"),
            lambda a: a["mic"].update(sha256=""),
            lambda a: a["rpmEvidence"].update(logId=1),
        ):
            self.packet = json.loads(original)
            mutate(self.packet["artifacts"][name])
            self.pin()
            self.assert_rejected(
                "mismatch|not proven|Missing|Unresolved|[Pp]roducer|timeline|parentId|minimal resolved"
            )
        self.assertEqual(self.downloads, [])

    def test_packet_pin_review_log_metadata_and_failed_evidence_task(self):
        self.args.evidence_sha256 = "0" * 64
        self.assert_rejected("hash mismatch")
        self.packet["review"]["reference"] = ""
        self.pin()
        self.assert_rejected("minimal resolved")
        self.packet["review"]["reference"] = "fixture"
        self.packet["logs"]["1"]["lineCount"] = 2
        self.pin()
        self.assert_rejected("log metadata mismatch")
        self.packet["logs"]["1"]["lineCount"] = 1
        self.records[1]["result"] = "failed"
        self.pin()
        self.assert_rejected("did not succeed")

    def test_missing_artifact_and_missing_zip_clone(self):
        saved = self.listing.pop()
        self.assert_rejected("Missing or ambiguous artifact")
        self.listing.append(saved)
        self.contents["trident-testimage"].pop("trident-testimage_1.cosi")
        self.assert_rejected("Expected exactly one")

    def test_changed_download_rejected_before_executing_storm(self):
        self.contents["go-tools"]["storm-trident"] = b"wrong bytes"
        self.assert_rejected("size mismatch")
        self.assertEqual(self.commands, [])

    def test_unsafe_zip_and_special_entries(self):
        for index, filename in enumerate(("../escape", "/absolute", "a\\b", "C:/bad")):
            self.args.output = self.root / f"unsafe-{index}"
            self.extra_entry = filename
            self.assert_rejected("Unsafe artifact ZIP")
        for mode in (stat.S_IFLNK, stat.S_IFIFO):
            archive = self.root / "special.zip"
            with zipfile.ZipFile(archive, "w") as output:
                entry = zipfile.ZipInfo("special")
                entry.external_attr = (mode | 0o777) << 16
                output.writestr(entry, b"target")
            with self.assertRaisesRegex(resolver.ado.ArtifactError, "Unsafe"):
                resolver.extract(archive, self.root / "unused", {})

    def test_duplicate_zip_basename_rejected(self):
        self.extra_entry = "duplicate/storm-trident"
        self.assert_rejected("Expected exactly one")

    def test_prepare_failure_or_missing_prepared_clone_never_publishes_manifest(self):
        with patch.object(
            self, "preparation", side_effect=subprocess.TimeoutExpired("fixture", 30)
        ):
            with self.assertRaises(subprocess.TimeoutExpired):
                self.prepare()
        self.assertFalse((self.args.output / "bundle/provenance.json").exists())
        self.args.output = self.root / "second"
        preparation = self.preparation

        def omit_clone(argv, **kwargs):
            result = preparation(argv, **kwargs)
            (Path(argv[11]) / "regular_v2.cosi").unlink()
            return result

        with patch.object(self, "preparation", side_effect=omit_clone):
            self.assert_rejected("Missing prepared file")

    def test_optional_public_certificate(self):
        self.args.include_uki_certificate = True
        name = "trident-usrverity-testimage"
        data = b"fixture public certificate only"
        self.contents[name]["ca_cert.pem"] = data
        self.packet["artifacts"][name]["files"]["ca_cert.pem"] = {
            "size": len(data),
            "sha256": hashlib.sha256(data).hexdigest(),
        }
        self.pin()
        path, pin = self.prepare()
        manifest = runner.verify_bundle(path.parent, path, pin, REVISION)
        self.assertEqual(
            manifest["signingCertificate"], "artifacts/usrverity/ca_cert.pem"
        )

    def test_explicit_identity_and_new_output_required(self):
        for field, value in (
            ("build_id", None),
            ("expected_definition_id", 5067),
            ("expected_revision", "main"),
            ("expected_ref", ""),
        ):
            previous = getattr(self.args, field)
            setattr(self.args, field, value)
            with self.assertRaises(resolver.ado.ArtifactError):
                resolver.producer(self.args)
            setattr(self.args, field, previous)
        self.args.output.mkdir()
        with self.assertRaises(FileExistsError):
            self.prepare()
        self.assertEqual(self.downloads, [])

    def test_raw_logs_unknown_fields_and_signed_urls_never_enter_handoff(self):
        original = json.dumps(self.packet)
        for mutate in (
            lambda p: p["logs"]["1"].update(text="fixture-unredacted-log"),
            lambda p: p.update(environment="fixture-environment"),
            lambda p: p["review"].update(reference="https://example.test/?sig=fixture"),
            lambda p: p["artifacts"]["trident-testimage"]["mic"].update(
                identity="https://example.test/image?sig=fixture"
            ),
        ):
            self.packet = json.loads(original)
            mutate(self.packet)
            self.pin()
            self.assert_rejected("raw logs are forbidden|minimal resolved")
        self.assertEqual(self.downloads, [])

    def test_cli_errors_do_not_echo_transport_or_packet_values(self):
        for value, expected in (
            ("HTTP 503: https://example.test/?sig=fixture-private", "HTTP 503"),
            ("fixture-private-environment", "ArtifactError"),
        ):
            with (
                patch.object(
                    sys,
                    "argv",
                    [
                        str(SCRIPT),
                        "--build-id",
                        "123",
                        "--expected-definition-id",
                        "5073",
                        "--expected-revision",
                        REVISION,
                        "--expected-ref",
                        "refs/heads/candidate",
                        "--evidence",
                        str(self.args.evidence),
                        "--evidence-sha256",
                        "a" * 64,
                        "--output",
                        str(self.args.output),
                        "--timeout-seconds",
                        "30",
                    ],
                ),
                patch.object(
                    resolver.ado, "get_ado_token", return_value="fixture-token"
                ),
                patch.object(
                    resolver, "prepare", side_effect=resolver.ado.ArtifactError(value)
                ),
                patch.object(sys, "stderr", new_callable=io.StringIO) as stderr,
            ):
                self.assertEqual(resolver.main(), 1)
            self.assertIn(expected, stderr.getvalue())
            self.assertNotIn("fixture-private", stderr.getvalue())

    def test_provider_health_errors_remain_visible(self):
        with patch.object(
            resolver.ado,
            "request_json",
            side_effect=resolver.ado.ArtifactError("HTTP 503"),
        ):
            with self.assertRaisesRegex(resolver.ado.ArtifactError, "HTTP 503"):
                resolver.prepare(self.args, "fixture-token")
        self.assertEqual(self.downloads, [])


if __name__ == "__main__":
    unittest.main()
