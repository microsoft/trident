import importlib.util
import io
import json
import os
import subprocess
import sys
import tempfile
import unittest
import urllib.error
import urllib.parse

from pathlib import Path
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parents[1] / "probe_vm_e2e_access.py"
SPEC = importlib.util.spec_from_file_location("access_probe", SCRIPT)
probe = importlib.util.module_from_spec(SPEC)
with patch.object(sys, "path", [str(SCRIPT.parent), *sys.path]):
    SPEC.loader.exec_module(probe)

PRIVATE = "fixture-private-value-never-real"


class AccessProbeTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.output = Path(self.temp.name) / "report"
        self.build = {
            "id": probe.BUILD_ID,
            "project": {"id": probe.PROJECT_ID},
            "definition": {"id": 5073, "revision": 7},
            "sourceVersion": probe.REVISION,
            "sourceBranch": probe.REF,
            "status": "completed",
            "result": "succeeded",
            "untrusted": PRIVATE,
        }
        self.timeline = {
            "records": [
                {
                    "type": "Stage",
                    "identifier": name,
                    "state": "completed",
                    "result": result,
                    "issues": [{"message": PRIVATE}],
                }
                for names, result in (
                    (probe.STAGES.values(), "succeeded"),
                    (probe.SKIPPED, "skipped"),
                )
                for name in names
            ]
        }
        self.artifacts = {
            name: {
                "id": artifact_id,
                "name": name,
                "resource": {
                    "type": "PipelineArtifact",
                    "properties": {"artifactsize": str(size)},
                    "downloadUrl": f"https://fixture.artifacts.visualstudio.com/data?sig={PRIVATE}",
                },
                "untrusted": PRIVATE,
            }
            for name, (artifact_id, size) in probe.ARTIFACTS.items()
        }
        self.requests = []
        self.cli_result = subprocess.CompletedProcess(
            [], 0, stdout=PRIVATE, stderr=PRIVATE
        )

    def request(self, url, token):
        self.assertEqual(token, PRIVATE)
        self.requests.append(url)
        parsed = urllib.parse.urlsplit(url)
        self.assertEqual(parsed.netloc, "dev.azure.com")
        self.assertTrue(
            parsed.path.startswith(
                f"/mariner-org/{probe.PROJECT_ID}/_apis/build/builds/{probe.BUILD_ID}"
            )
        )
        query = urllib.parse.parse_qs(parsed.query)
        self.assertEqual(query["api-version"], ["7.1"])
        if parsed.path.endswith("/timeline"):
            return self.timeline
        if parsed.path.endswith("/artifacts"):
            return self.artifacts[query["artifactName"][0]]
        self.assertNotIn("latest", url)
        return self.build

    def run_probe(self, cli_error=None, request_error=None, available=True):
        with (
            patch.object(
                probe.shutil, "which", return_value="/fixture/az" if available else None
            ),
            patch.object(
                probe.ado.subprocess,
                "run",
                side_effect=cli_error,
                return_value=self.cli_result,
            ) as cli,
            patch.object(
                probe.ado, "request_json", side_effect=request_error or self.request
            ),
            patch.object(
                probe.ado,
                "download_file",
                side_effect=AssertionError("Content downloads forbidden"),
            ) as download,
            patch.object(sys, "stdout", new_callable=io.StringIO) as stdout,
            patch.object(sys, "stderr", new_callable=io.StringIO) as stderr,
        ):
            status = probe.probe(self.output)
        text = (self.output / "summary.json").read_text()
        self.assertNotIn(PRIVATE, text + stdout.getvalue() + stderr.getvalue())
        self.assertNotIn("downloadUrl", text)
        download.assert_not_called()
        return status, json.loads(text), cli

    def test_existing_credential_exact_graph_and_six_metadata_reads_only(self):
        status, report, cli = self.run_probe()
        self.assertEqual(status, 0)
        self.assertEqual(report["outcome"], "passed")
        self.assertEqual(len(report["checks"]), 10)
        self.assertTrue(all(c["outcome"] == "passed" for c in report["checks"]))
        self.assertEqual(len(report["stages"]["succeeded"]), 7)
        self.assertEqual(len(report["stages"]["skipped"]), 41)
        self.assertEqual(len(report["artifacts"]), 6)
        self.assertEqual(len(self.requests), 8)
        self.assertEqual(report["artifactContentAccess"], "not_tested")
        self.assertFalse(report["provenanceQualified"])
        self.assertFalse(report["vmExecuted"])
        self.assertEqual(report["credentialPrerequisite"], "usable")
        self.assertFalse(report["credentialInitializationPerformedByProbe"])
        cli.assert_called_once_with(
            [
                "/fixture/az",
                "account",
                "get-access-token",
                "--resource",
                probe.ado.ADO_RESOURCE,
                "--query",
                "accessToken",
                "-o",
                "tsv",
            ],
            check=True,
            capture_output=True,
            text=True,
        )

    def test_missing_cli_fails_before_credential_or_network(self):
        status, report, cli = self.run_probe(available=False)
        self.assertEqual(status, 1)
        self.assertEqual(
            report["checks"][-1]["diagnostic"]["code"], "azure_cli_unavailable"
        )
        cli.assert_not_called()
        self.assertEqual(self.requests, [])

    def test_missing_existing_session_or_empty_token_fails_without_fallback(self):
        for index, error in enumerate(
            (
                subprocess.CalledProcessError(
                    1, ["az"], output=PRIVATE, stderr=PRIVATE
                ),
                None,
            )
        ):
            self.output = Path(self.temp.name) / f"credential-{index}"
            if error is None:
                self.cli_result.stdout = ""
            status, report, cli = self.run_probe(cli_error=error)
            self.assertEqual(status, 1)
            self.assertEqual(report["checks"][-1]["name"], "existingCredential")
            self.assertEqual(
                report["checks"][-1]["diagnostic"]["code"],
                "prerequisite_credential_not_initialized_or_unavailable",
            )
            self.assertEqual(
                report["credentialPrerequisite"], "not_initialized_or_unavailable"
            )
            self.assertEqual(report["adoAuthorization"], "not_tested")
            self.assertEqual(cli.call_count, 1)
        self.assertEqual(self.requests, [])

    def test_wrong_producer_fields_rejected_without_artifact_reads(self):
        original = json.dumps(self.build)
        for index, mutation in enumerate(
            (
                {"id": 1266406},
                {"project": {"id": "other"}},
                {"definition": {"id": 5067, "revision": 7}},
                {"definition": {"id": 5073, "revision": 8}},
                {"sourceVersion": "b" * 40},
                {"sourceBranch": "refs/heads/main"},
                {"status": "inProgress"},
                {"result": "failed"},
            )
        ):
            self.output = Path(self.temp.name) / f"producer-{index}"
            self.build = {**json.loads(original), **mutation}
            status, report, _ = self.run_probe()
            self.assertEqual(status, 1)
            self.assertEqual(report["checks"][-1]["name"], "producerIdentity")
            self.assertEqual(report["artifacts"], [])

    def test_exact_stage_partition_rejects_missing_duplicate_unknown_and_wrong_results(
        self,
    ):
        original = json.dumps(self.timeline)
        mutations = (
            lambda r: r.pop(),
            lambda r: r.append(dict(r[0])),
            lambda r: r[0].update(identifier="UnexpectedStage"),
            lambda r: r[0].update(result="succeededWithIssues"),
            lambda r: r[7].update(result="succeeded"),
            lambda r: r[0].update(state="inProgress"),
        )
        for index, mutate in enumerate(mutations):
            self.output = Path(self.temp.name) / f"timeline-{index}"
            self.timeline = json.loads(original)
            mutate(self.timeline["records"])
            status, report, _ = self.run_probe()
            self.assertEqual(status, 1)
            self.assertEqual(report["checks"][-1]["name"], "stagePartition")
            self.assertEqual(report["artifacts"], [])

    def test_artifact_metadata_mismatches_fail_closed(self):
        original = json.dumps(self.artifacts)
        for index, mutate in enumerate(
            (
                lambda a: a.update(name="other"),
                lambda a: a.update(id=1),
                lambda a: a["resource"]["properties"].update(artifactsize="1"),
                lambda a: a["resource"].update(type="Container"),
                lambda a: a["resource"].update(
                    downloadUrl=f"https://untrusted.test/?sig={PRIVATE}"
                ),
            )
        ):
            self.output = Path(self.temp.name) / f"artifact-{index}"
            self.artifacts = json.loads(original)
            mutate(self.artifacts["go-tools"])
            status, report, _ = self.run_probe()
            self.assertEqual(status, 1)
            self.assertEqual(report["checks"][-1]["name"], "artifactMetadata:go-tools")
            self.assertEqual(report["artifacts"], [])

    def test_http_access_and_provider_failures_are_visible_without_bodies(self):
        for code in (401, 403, 404, 429, 503):
            self.output = Path(self.temp.name) / f"http-{code}"
            error = probe.ado.ArtifactError(PRIVATE)
            error.__cause__ = urllib.error.HTTPError(
                f"https://example.test/?sig={PRIVATE}", code, PRIVATE, {}, None
            )
            error.__cause__.close()
            status, report, _ = self.run_probe(request_error=error)
            self.assertEqual(status, 1)
            self.assertEqual(report["checks"][-1]["diagnostic"]["httpStatus"], code)
            if code == 403:
                self.assertEqual(report["adoAuthorization"], "denied")
                self.assertEqual(report["credentialPrerequisite"], "usable")

    def test_deadline_and_unexpected_failure_retain_incremental_safe_report(self):
        for index, error in enumerate((TimeoutError(PRIVATE), RuntimeError(PRIVATE))):
            self.output = Path(self.temp.name) / f"failure-{index}"

            def fail(url, token):
                current = json.loads((self.output / "summary.json").read_text())
                self.assertEqual(current["checks"][-1]["outcome"], "started")
                raise error

            status, report, _ = self.run_probe(request_error=fail)
            self.assertEqual(status, 1)
            self.assertEqual(report["outcome"], "failed")
            self.assertEqual(report["checks"][-1]["outcome"], "failed")

    def test_report_directory_must_be_new(self):
        self.output.mkdir()
        with self.assertRaises(FileExistsError):
            probe.probe(self.output)

    def test_workflow_is_branch_specific_pinned_and_report_only(self):
        workflow = (
            SCRIPT.parents[2] / ".github/workflows/vm-e2e-access-validation.yml"
        ).read_text()
        for expected in (
            "user/britel/vm-e2e-access-validation-20261123",
            "runs-on: trident-github-runner-ubuntu2204-amd64",
            "timeout-minutes: 10",
            "persist-credentials: false",
            "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1",
            "actions/upload-artifact@cf430e030ddbb5b0abf93d22962f4752f3646cd9",
            "${{ env.REPORT_DIR }}/identity-initialization.json",
            "${{ env.REPORT_DIR }}/probe/summary.json",
            "${{ env.REPORT_DIR }}/workflow-status.json",
        ):
            self.assertIn(expected, workflow)
        self.assertEqual(workflow.count("if: always()"), 2)
        self.assertNotIn("continue-on-error", workflow)
        self.assertLess(
            workflow.index("Initialize only the already-approved runner UMI"),
            workflow.index("Probe initialized caller access"),
        )
        for forbidden in (
            "workflow_dispatch",
            "az login",
            "ADO_UMI",
            "setup-go",
            "setup.sh",
            "libvirt",
            "docker",
            "secrets.",
            "id-token: write",
        ):
            self.assertNotIn(forbidden, workflow)

    def test_exact_workflow_initialization_is_silent_bounded_and_fail_closed(self):
        workflow = (
            SCRIPT.parents[2] / ".github/workflows/vm-e2e-access-validation.yml"
        ).read_text()
        lines = workflow.splitlines()
        start = lines.index("          python3 - <<'PY'") + 1
        end = lines.index("          PY", start)
        code = compile(
            "\n".join(line[10:] for line in lines[start:end]),
            "<workflow-identity-initialization>",
            "exec",
        )
        for index, (result, error, expected, diagnostic) in enumerate(
            (
                (0, None, 0, "runner_identity_initialized"),
                (1, None, 1, "runner_identity_initialization_failed"),
                (None, FileNotFoundError(PRIVATE), 1, "azure_cli_unavailable"),
                (
                    None,
                    subprocess.TimeoutExpired("az", 60, output=PRIVATE, stderr=PRIVATE),
                    1,
                    "runner_identity_initialization_timeout",
                ),
                (
                    None,
                    RuntimeError(PRIVATE),
                    1,
                    "runner_identity_initialization_execution_failed",
                ),
            )
        ):
            output = Path(self.temp.name) / f"init-{index}"
            with (
                patch.dict(os.environ, {"REPORT_DIR": str(output)}),
                patch.object(
                    subprocess,
                    "run",
                    side_effect=error,
                    return_value=subprocess.CompletedProcess(
                        [], result, PRIVATE, PRIVATE
                    ),
                ) as run,
                patch.object(sys, "stdout", new_callable=io.StringIO) as stdout,
                patch.object(sys, "stderr", new_callable=io.StringIO) as stderr,
                self.assertRaises(SystemExit) as exit_status,
            ):
                exec(code, {})
            self.assertEqual(exit_status.exception.code, expected)
            report_text = (output / "identity-initialization.json").read_text()
            report = json.loads(report_text)
            self.assertEqual(report["code"], diagnostic)
            self.assertEqual(report["outcome"], "passed" if expected == 0 else "failed")
            self.assertNotIn(
                PRIVATE, report_text + stdout.getvalue() + stderr.getvalue()
            )
            run.assert_called_once_with(
                [
                    "az",
                    "login",
                    "--identity",
                    "--client-id",
                    "e2fd61e6-b160-4f02-a5ac-3d949a4fe01d",
                    "--allow-no-subscriptions",
                ],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                timeout=60,
                check=False,
            )


if __name__ == "__main__":
    unittest.main()
