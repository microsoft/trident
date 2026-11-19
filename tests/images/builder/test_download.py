import copy
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from builder import (
    BaseImageData,
    ImageConfig,
    OutputFormat,
    PipelineArtifactBaseImageManifest,
)
from builder import download
from builder.builder import build_one


def manifest(directory):
    return PipelineArtifactBaseImageManifest(
        image=BaseImageData("acl-t", directory / "acl.vhdx"),
        pipeline_id=5304,
        artifact_name="acl-template",
        filename="acl.vhdx",
        repository_name="acl-pipelines",
        repository_refs={
            "self": "refs/heads/main",
            "azure_container_linux": "refs/heads/aclmain",
        },
    )


def build(run_id=1003, **overrides):
    result = {
        "id": run_id,
        "definition": {"id": 5304},
        "buildNumber": f"acl.{run_id}",
        "reason": "schedule",
        "status": "completed",
        "result": "succeeded",
        "sourceBranch": "refs/heads/main",
        "repository": {"name": "acl-pipelines"},
    }
    result.update(overrides)
    return result


def run(run_id=1003, **refs):
    repositories = {
        "self": {"refName": "refs/heads/main", "version": "a" * 40},
        "azure_container_linux": {"refName": "refs/heads/aclmain", "version": "b" * 40},
    }
    for name, ref in refs.items():
        repositories[name]["refName"] = ref
    return {
        "id": run_id,
        "pipeline": {"id": 5304},
        "state": "completed",
        "result": "succeeded",
        "resources": {"repositories": repositories},
    }


class PipelineSourceTests(unittest.TestCase):
    def test_selects_latest_scheduled_success_matching_both_actual_branches(self):
        candidates = [
            build(1010, reason="manual"),
            build(1009, result="failed"),
            build(1008, sourceBranch="refs/heads/feature"),
            build(1007, repository={"name": "other-repo"}),
            build(1006),
            build(1005),
            build(1004),
            build(1003),
            build(1002),
        ]
        resources = {
            1006: run(1006, self="refs/heads/feature"),
            1005: run(1005, azure_container_linux="refs/heads/feature"),
            1004: run(1004),
            1003: run(1003),
        }
        del resources[1004]["resources"]["repositories"]["azure_container_linux"]
        inspected = []

        def response(image, args):
            if args[:3] == ["pipelines", "runs", "list"]:
                return candidates
            run_id = int(
                next(arg.split("=")[1] for arg in args if arg.startswith("runId="))
            )
            inspected.append(run_id)
            return resources[run_id]

        with patch.object(download, "_az_json", side_effect=response):
            selected = download.select_pipeline_run(manifest(Path("unused")))
        self.assertEqual(selected["runId"], 1003)
        self.assertEqual(inspected, [1006, 1005, 1004, 1003])
        self.assertEqual(
            selected["repositories"],
            resources[1003]["resources"]["repositories"],
        )

    def test_no_matching_run_fails_closed(self):
        with patch.object(
            download,
            "_az_json",
            side_effect=[[build()], run(azure_container_linux="refs/heads/other")],
        ):
            with self.assertRaisesRegex(RuntimeError, "No successful scheduled run"):
                download.select_pipeline_run(manifest(Path("unused")))

    def test_api_failure_is_not_treated_as_a_branch_mismatch(self):
        failure = subprocess.CalledProcessError(1, ["az", "devops", "invoke"])
        with patch.object(download, "_az_json", side_effect=[[build()], failure]):
            with self.assertRaises(subprocess.CalledProcessError):
                download.select_pipeline_run(manifest(Path("unused")))

    def test_download_records_selected_run_and_actual_content_hash(self):
        with tempfile.TemporaryDirectory() as directory:
            image = manifest(Path(directory))
            image.image.path.write_bytes(b"old")
            payload = b"selected ACL image"

            def artifact(args, **kwargs):
                destination = Path(args[args.index("--path") + 1])
                (destination / image.filename).write_bytes(payload)

            source = {
                "runId": 1003,
                "buildNumber": "acl.1003",
                "repositories": run()["resources"]["repositories"],
            }
            with patch.object(
                download, "select_pipeline_run", return_value=copy.deepcopy(source)
            ):
                with patch.object(download.subprocess, "run", side_effect=artifact):
                    download.download_pipeline_artifact(image)
            provenance = json.loads(
                image.image.path.with_suffix(".source.json").read_text()
            )
            self.assertEqual(image.image.path.read_bytes(), payload)
            self.assertEqual(provenance["runId"], source["runId"])
            self.assertEqual(provenance["repositories"], source["repositories"])
            self.assertEqual(provenance["sha256"], hashlib.sha256(payload).hexdigest())

    def test_missing_or_ambiguous_artifact_preserves_previous_cache(self):
        for count in (0, 2):
            with self.subTest(
                matches=count
            ), tempfile.TemporaryDirectory() as directory:
                image = manifest(Path(directory))
                image.image.path.write_bytes(b"old image")
                provenance = image.image.path.with_suffix(".source.json")
                provenance.write_text('{"runId": 999}')

                def artifact(args, **kwargs):
                    destination = Path(args[args.index("--path") + 1])
                    for index in range(count):
                        nested = destination / str(index)
                        nested.mkdir()
                        (nested / image.filename).write_bytes(b"ambiguous")

                with patch.object(
                    download,
                    "select_pipeline_run",
                    return_value={"runId": 1003, "buildNumber": "acl.1003"},
                ):
                    with patch.object(download.subprocess, "run", side_effect=artifact):
                        with self.assertRaisesRegex(
                            RuntimeError, "Expected exactly one"
                        ):
                            download.download_pipeline_artifact(image)
                self.assertEqual(image.image.path.read_bytes(), b"old image")
                self.assertEqual(provenance.read_text(), '{"runId": 999}')


class ImageProvenanceTests(unittest.TestCase):
    def test_cache_tracks_source_identity_and_rejects_corrupt_input(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "config").mkdir()
            (root / "config/image.yaml").write_text("os: {}\n")
            base = root / "input.vhdx"
            base.write_bytes(b"ACL template")
            source_path = base.with_suffix(".source.json")
            provenance = {"runId": 1003, "sha256": download.file_sha256(base)}
            source_path.write_text(json.dumps(provenance))
            output = root / "output"
            output.mkdir()
            image = ImageConfig(
                "acl",
                source=str(root),
                config="config",
                config_file=Path("image.yaml"),
                base_image=BaseImageData("input", base),
                requires_trident=False,
                output_format=OutputFormat.QCOW2,
            )

            def customize(*args, **kwargs):
                args[5].write_bytes(b"customized image")

            def build_image():
                build_one("unused", image, output, [], None, False, False)

            output_source = (output / image.file_name()).with_suffix(".source.json")
            with patch(
                "builder.builder.customize.build_config", side_effect=customize
            ) as customizer:
                build_image()
                self.assertEqual(output_source.read_bytes(), source_path.read_bytes())
                build_image()
                self.assertEqual(customizer.call_count, 1)
                output_source.unlink()
                build_image()
                self.assertEqual(customizer.call_count, 2)
                provenance["runId"] = 1004
                source_path.write_text(json.dumps(provenance))
                os.utime(source_path, (0, 0))
                build_image()
                self.assertEqual(customizer.call_count, 3)
                self.assertEqual(output_source.read_bytes(), source_path.read_bytes())
                base.write_bytes(b"corrupt template")
                with self.assertRaisesRegex(RuntimeError, "does not match provenance"):
                    build_image()
                self.assertEqual(customizer.call_count, 3)
