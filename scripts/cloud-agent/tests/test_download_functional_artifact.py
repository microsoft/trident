import importlib.util
import json
import tempfile
import threading
import time
import unittest
import urllib.parse
import zipfile

from pathlib import Path
from concurrent.futures import ThreadPoolExecutor
from unittest.mock import patch

MODULE_PATH = Path(__file__).resolve().parents[1] / "download_functional_artifact.py"
SPEC = importlib.util.spec_from_file_location(
    "download_functional_artifact", MODULE_PATH
)
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class FunctionalArtifactTests(unittest.TestCase):
    def setUp(self):
        self.config = {
            "schemaVersion": 2,
            "organization": "mariner-org",
            "project": "ECF",
            "definitionId": 5067,
            "sourceBranch": "refs/heads/main",
            "buildStatus": "completed",
            "buildResult": "succeeded",
            "artifactName": "trident-functest",
            "artifactType": "PipelineArtifact",
            "imageFilename": "trident-functest.qcow2",
        }

    def build(self, build_id=1244644):
        return {
            "id": build_id,
            "definition": {"id": 5067},
            "status": "completed",
            "result": "succeeded",
            "sourceBranch": "refs/heads/main",
            "sourceVersion": "abc123",
        }

    def artifact(self):
        return {
            "id": 42,
            "name": "trident-functest",
            "resource": {
                "type": "PipelineArtifact",
                "properties": {"artifactsize": "10"},
                "downloadUrl": (
                    "https://example.artifacts.visualstudio.com/path/content"
                ),
            },
        }

    def test_verify_build_rejects_selection_mismatch(self):
        mismatches = {
            "definition.id": ("definition", {"id": 9999}),
            "status": ("status", "inProgress"),
            "result": ("result", "failed"),
            "sourceBranch": ("sourceBranch", "refs/heads/feature"),
        }
        for expected_error, (field, value) in mismatches.items():
            with self.subTest(field=field):
                build = self.build()
                build[field] = value
                with self.assertRaisesRegex(MODULE.ArtifactError, expected_error):
                    MODULE.verify_build(self.config, build)

    def test_resolve_build_selects_latest_successful_main_build(self):
        build = self.build()
        requested_urls = []

        def request_json(url, token):
            requested_urls.append(url)
            self.assertEqual(token, "token")
            return {"value": [build]}

        original_request_json = MODULE.request_json
        MODULE.request_json = request_json
        self.addCleanup(setattr, MODULE, "request_json", original_request_json)

        self.assertEqual(MODULE.resolve_build(self.config, "token", None), build)
        query = urllib.parse.parse_qs(urllib.parse.urlparse(requested_urls[0]).query)
        self.assertEqual(query["definitions"], ["5067"])
        self.assertEqual(query["branchName"], ["refs/heads/main"])
        self.assertEqual(query["statusFilter"], ["completed"])
        self.assertEqual(query["resultFilter"], ["succeeded"])
        self.assertEqual(query["queryOrder"], ["finishTimeDescending"])
        self.assertEqual(query["$top"], ["1"])

    def test_resolve_build_uses_explicit_override(self):
        build = self.build(7654321)
        requested_urls = []

        def request_json(url, token):
            requested_urls.append(url)
            return build

        original_request_json = MODULE.request_json
        MODULE.request_json = request_json
        self.addCleanup(setattr, MODULE, "request_json", original_request_json)

        self.assertEqual(MODULE.resolve_build(self.config, "token", 7654321), build)
        self.assertIn("/build/builds/7654321?", requested_urls[0])

    def test_verify_artifact_returns_direct_download_url(self):
        artifact = self.artifact()

        self.assertEqual(
            MODULE.verify_artifact(self.config, artifact),
            artifact["resource"]["downloadUrl"],
        )

    def test_find_image_entry_rejects_ambiguous_archive(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            archive_path = Path(temp_dir) / "artifact.zip"
            with zipfile.ZipFile(archive_path, "w") as archive:
                archive.writestr("a/trident-functest.qcow2", b"first")
                archive.writestr("b/trident-functest.qcow2", b"second")

            with zipfile.ZipFile(archive_path) as archive:
                with self.assertRaisesRegex(MODULE.ArtifactError, "exactly one"):
                    MODULE.find_image_entry(archive, "trident-functest.qcow2")

    def test_extract_image_verifies_size_and_hash(self):
        image = b"functional image bytes"

        with tempfile.TemporaryDirectory() as temp_dir:
            temp_path = Path(temp_dir)
            archive_path = temp_path / "artifact.zip"
            image_path = temp_path / "trident-functest.qcow2"
            with zipfile.ZipFile(archive_path, "w") as archive:
                archive.writestr("trident-functest/trident-functest.qcow2", image)

            image_size, image_sha256 = MODULE.extract_image(
                self.config, archive_path, image_path
            )

            self.assertEqual(image_path.read_bytes(), image)
            self.assertEqual(image_size, len(image))
            self.assertEqual(image_sha256, MODULE.hashlib.sha256(image).hexdigest())
            self.assertTrue(MODULE.image_matches(image_path, image_size, image_sha256))

    def test_prepare_artifact_caches_by_build_and_records_computed_hash(self):
        image = b"fresh functional image"
        build = self.build(7654321)
        artifact = self.artifact()
        download_count = 0

        def request_json(url, token):
            if "/artifacts?" in url:
                return artifact
            return build

        def download_file(url, token, destination):
            nonlocal download_count
            download_count += 1
            with zipfile.ZipFile(destination, "w") as archive:
                archive.writestr("trident-functest/trident-functest.qcow2", image)

        original_request_json = MODULE.request_json
        original_download_file = MODULE.download_file
        MODULE.request_json = request_json
        MODULE.download_file = download_file
        self.addCleanup(setattr, MODULE, "request_json", original_request_json)
        self.addCleanup(setattr, MODULE, "download_file", original_download_file)

        with tempfile.TemporaryDirectory() as temp_dir:
            temp_path = Path(temp_dir)
            provenance_path = MODULE.prepare_artifact(
                self.config,
                temp_path / "cache",
                temp_path / "artifacts" / "trident-functest.qcow2",
                "token",
                build_id=7654321,
            )
            provenance = json.loads(provenance_path.read_text(encoding="utf-8"))

            self.assertEqual(provenance_path.parent.name, "build-7654321")
            self.assertEqual(provenance["build"]["id"], 7654321)
            self.assertEqual(provenance["image"]["size"], len(image))
            self.assertEqual(
                provenance["image"]["sha256"],
                MODULE.hashlib.sha256(image).hexdigest(),
            )

            provenance["build"].pop("status")
            provenance_path.write_text(
                json.dumps(provenance, indent=2) + "\n", encoding="utf-8"
            )
            second_provenance_path = MODULE.prepare_artifact(
                self.config,
                temp_path / "cache",
                temp_path / "artifacts" / "trident-functest.qcow2",
                "token",
                build_id=7654321,
            )
            second_provenance = json.loads(
                second_provenance_path.read_text(encoding="utf-8")
            )
            self.assertEqual(download_count, 1)
            self.assertTrue(second_provenance["cacheHit"])

    def test_prepare_artifact_keeps_cache_reusable_when_link_fails(self):
        image = b"fresh functional image"
        build = self.build(7654321)
        artifact = self.artifact()
        download_count = 0

        def request_json(url, token):
            if "/artifacts?" in url:
                return artifact
            return build

        def download_file(url, token, destination):
            nonlocal download_count
            download_count += 1
            with zipfile.ZipFile(destination, "w") as archive:
                archive.writestr("trident-functest/trident-functest.qcow2", image)

        original_request_json = MODULE.request_json
        original_download_file = MODULE.download_file
        MODULE.request_json = request_json
        MODULE.download_file = download_file
        self.addCleanup(setattr, MODULE, "request_json", original_request_json)
        self.addCleanup(setattr, MODULE, "download_file", original_download_file)

        with tempfile.TemporaryDirectory() as temp_dir:
            temp_path = Path(temp_dir)
            output_root = temp_path / "cache"
            link_path = temp_path / "artifacts" / "trident-functest.qcow2"
            link_path.parent.mkdir(parents=True)
            link_path.write_bytes(b"existing file")

            with self.assertRaisesRegex(MODULE.ArtifactError, "existing path"):
                MODULE.prepare_artifact(
                    self.config,
                    output_root,
                    link_path,
                    "token",
                    build_id=7654321,
                )

            provenance_path = output_root / "build-7654321" / "provenance.json"
            self.assertTrue(provenance_path.is_file())

            link_path.unlink()
            MODULE.prepare_artifact(
                self.config,
                output_root,
                link_path,
                "token",
                build_id=7654321,
            )

            self.assertEqual(download_count, 1)
            self.assertTrue(link_path.is_symlink())

    def test_prepare_artifact_recovers_interrupted_provenance_without_touching_local_image(
        self,
    ):
        image = b"functional image"
        build = self.build()
        artifact = self.artifact()

        def request_json(url, token):
            return artifact if "/artifacts?" in url else build

        def download_file(url, token, destination):
            with zipfile.ZipFile(destination, "w") as archive:
                archive.writestr(self.config["imageFilename"], image)

        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            local_image = root / "artifacts" / self.config["imageFilename"]
            local_image.parent.mkdir()
            local_image.write_bytes(b"developer-built image")
            with patch.object(
                MODULE, "request_json", side_effect=request_json
            ), patch.object(
                MODULE, "download_file", side_effect=download_file
            ) as download:
                with patch.object(
                    MODULE, "write_provenance", side_effect=OSError("interrupted")
                ):
                    with self.assertRaisesRegex(OSError, "interrupted"):
                        MODULE.prepare_artifact(
                            self.config,
                            root / "cache",
                            None,
                            "token",
                            build_id=build["id"],
                        )
                provenance_path = MODULE.prepare_artifact(
                    self.config, root / "cache", None, "token", build_id=build["id"]
                )
                self.assertEqual(download.call_count, 2)
            self.assertEqual(local_image.read_bytes(), b"developer-built image")
            self.assertEqual(
                json.loads(provenance_path.read_text())["image"]["sha256"],
                MODULE.hashlib.sha256(image).hexdigest(),
            )

    def test_prepare_artifact_serializes_concurrent_preparation(self):
        ready = threading.Barrier(2)
        artifact = self.artifact()
        build = self.build()

        def request_json(url, token):
            if "/artifacts?" in url:
                ready.wait(timeout=5)
                return artifact
            return build

        def download_file(url, token, destination):
            time.sleep(0.1)
            with zipfile.ZipFile(destination, "w") as archive:
                archive.writestr(self.config["imageFilename"], b"functional image")

        with tempfile.TemporaryDirectory() as temp_dir:
            with patch.object(
                MODULE, "request_json", side_effect=request_json
            ), patch.object(
                MODULE, "download_file", side_effect=download_file
            ) as download:
                with ThreadPoolExecutor(max_workers=2) as executor:
                    results = list(
                        executor.map(
                            lambda _: MODULE.prepare_artifact(
                                self.config,
                                Path(temp_dir),
                                None,
                                "token",
                                build_id=build["id"],
                            ),
                            range(2),
                        )
                    )
                self.assertEqual(results[0], results[1])
                self.assertEqual(download.call_count, 1)

    def test_temporary_paths_are_unique(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            temp_path = Path(temp_dir)
            first = MODULE.create_temporary_path(temp_path, ".image.", ".tmp")
            second = MODULE.create_temporary_path(temp_path, ".image.", ".tmp")
            self.addCleanup(first.unlink, missing_ok=True)
            self.addCleanup(second.unlink, missing_ok=True)

            self.assertNotEqual(first, second)

    def test_load_config_rejects_unknown_schema(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            config_path = Path(temp_dir) / "config.json"
            config_path.write_text(json.dumps({"schemaVersion": 3}), encoding="utf-8")

            with self.assertRaisesRegex(MODULE.ArtifactError, "Unsupported"):
                MODULE.load_config(config_path)

    def test_ensure_link_uses_image_target(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            temp_path = Path(temp_dir)
            image_path = temp_path / "cache" / "image.qcow2"
            link_path = temp_path / "artifacts" / "image.qcow2"
            image_path.parent.mkdir()
            image_path.write_bytes(b"image")

            MODULE.ensure_link(image_path.resolve(), link_path)
            MODULE.ensure_link(image_path.resolve(), link_path)

            self.assertTrue(link_path.is_symlink())
            self.assertEqual(link_path.resolve(), image_path.resolve())

            replacement_path = temp_path / "cache" / "replacement.qcow2"
            replacement_path.write_bytes(b"replacement")
            MODULE.ensure_link(replacement_path.resolve(), link_path)

            self.assertTrue(link_path.is_symlink())
            self.assertEqual(link_path.resolve(), replacement_path.resolve())


if __name__ == "__main__":
    unittest.main()
