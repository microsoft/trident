import importlib.util
import json
import tempfile
import unittest
import zipfile

from pathlib import Path
from unittest import mock

MODULE_PATH = Path(__file__).resolve().parents[1] / "download_install_artifacts.py"
SPEC = importlib.util.spec_from_file_location("download_install_artifacts", MODULE_PATH)
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class InstallArtifactTests(unittest.TestCase):
    def setUp(self):
        self.payload = b"pinned install payload"
        self.artifact = {
            "artifactId": 123,
            "artifactName": "installer",
            "artifactType": "PipelineArtifact",
            "artifactSize": 100,
            "archivePath": "installer/image.iso",
            "payloadSize": len(self.payload),
            "payloadSha256": MODULE.hashlib.sha256(self.payload).hexdigest(),
            "cacheFilename": "image.iso",
            "destinationPath": "artifacts/iso/image.iso",
        }
        self.config = {
            "schemaVersion": 1,
            "organization": "mariner-org",
            "project": "ECF",
            "definitionId": 5067,
            "buildId": 1244644,
            "sourceBranch": "refs/heads/main",
            "sourceVersion": "abc123",
            "buildResult": "succeeded",
            "artifacts": [self.artifact],
        }

    def write_config(self, root: Path, config=None) -> Path:
        config_path = root / "install-artifacts.json"
        config_path.write_text(json.dumps(config or self.config), encoding="utf-8")
        return config_path

    def test_load_config_rejects_destination_traversal(self):
        self.artifact["destinationPath"] = "../outside"
        with tempfile.TemporaryDirectory() as temp_dir:
            config_path = self.write_config(Path(temp_dir))
            with self.assertRaisesRegex(MODULE.ArtifactError, "within the repository"):
                MODULE.load_config(config_path)

    def test_load_config_rejects_duplicate_destination(self):
        self.config["artifacts"].append(dict(self.artifact))
        with tempfile.TemporaryDirectory() as temp_dir:
            config_path = self.write_config(Path(temp_dir))
            with self.assertRaisesRegex(MODULE.ArtifactError, "Duplicate"):
                MODULE.load_config(config_path)

    def test_find_payload_entry_requires_exact_path(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            archive_path = Path(temp_dir) / "artifact.zip"
            with zipfile.ZipFile(archive_path, "w") as archive:
                archive.writestr("other/image.iso", self.payload)

            with zipfile.ZipFile(archive_path) as archive:
                with self.assertRaisesRegex(MODULE.ArtifactError, "exactly one"):
                    MODULE.find_payload_entry(archive, "installer/image.iso")

    def test_find_payload_entry_rejects_unsafe_archive(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            archive_path = Path(temp_dir) / "artifact.zip"
            with zipfile.ZipFile(archive_path, "w") as archive:
                archive.writestr("../image.iso", self.payload)

            with zipfile.ZipFile(archive_path) as archive:
                with self.assertRaisesRegex(MODULE.ArtifactError, "Unsafe path"):
                    MODULE.find_payload_entry(archive, "installer/image.iso")

    def test_extract_payload_verifies_size_and_hash(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            temp_path = Path(temp_dir)
            archive_path = temp_path / "artifact.zip"
            payload_path = temp_path / "image.iso"
            with zipfile.ZipFile(archive_path, "w") as archive:
                archive.writestr(self.artifact["archivePath"], self.payload)

            MODULE.extract_payload(self.artifact, archive_path, payload_path)

            self.assertEqual(payload_path.read_bytes(), self.payload)
            self.assertTrue(MODULE.payload_matches(self.artifact, payload_path))

    def test_verify_artifact_id_rejects_mismatch(self):
        with self.assertRaisesRegex(MODULE.ArtifactError, "artifact.id"):
            MODULE.verify_artifact_id(self.artifact, {"id": 456})

    def test_write_provenance_records_all_payloads(self):
        build = {
            "id": 1244644,
            "buildNumber": "build-number",
            "definition": {"id": 5067},
            "result": "succeeded",
            "sourceBranch": "refs/heads/main",
            "sourceVersion": "abc123",
        }
        prepared = [{"name": "installer", "payload": {"sha256": "abc"}}]
        with tempfile.TemporaryDirectory() as temp_dir:
            provenance_path = Path(temp_dir) / "provenance.json"
            MODULE.write_provenance(self.config, build, prepared, provenance_path)
            provenance = json.loads(provenance_path.read_text(encoding="utf-8"))

            self.assertEqual(provenance["build"]["id"], 1244644)
            self.assertEqual(provenance["artifacts"], prepared)

    def test_prepare_artifacts_reuses_archive_for_multiple_payloads(self):
        second_payload = b"pinned update payload"
        second_artifact = {
            **self.artifact,
            "archivePath": "installer/update.cosi",
            "payloadSize": len(second_payload),
            "payloadSha256": MODULE.hashlib.sha256(second_payload).hexdigest(),
            "cacheFilename": "update.cosi",
            "destinationPath": "artifacts/update.cosi",
        }
        self.config["artifacts"].append(second_artifact)
        build = {
            "id": 1244644,
            "buildNumber": "build-number",
            "definition": {"id": 5067},
            "result": "succeeded",
            "sourceBranch": "refs/heads/main",
            "sourceVersion": "abc123",
        }
        artifact = {
            "id": 123,
            "name": "installer",
            "resource": {
                "type": "PipelineArtifact",
                "properties": {"artifactsize": "100"},
            },
        }
        download_count = 0

        def fake_request_json(url, _token):
            return build if "artifacts?" not in url else artifact

        def fake_download(_url, _token, destination):
            nonlocal download_count
            download_count += 1
            with zipfile.ZipFile(destination, "w") as archive:
                archive.writestr(self.artifact["archivePath"], self.payload)
                archive.writestr(second_artifact["archivePath"], second_payload)

        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            output_root = root / "cache"
            repository_root = root / "repo"
            repository_root.mkdir()
            with (
                mock.patch.object(
                    MODULE, "request_json", side_effect=fake_request_json
                ),
                mock.patch.object(MODULE, "verify_build"),
                mock.patch.object(
                    MODULE, "verify_artifact", return_value="download-url"
                ),
                mock.patch.object(MODULE, "download_file", side_effect=fake_download),
            ):
                MODULE.prepare_artifacts(
                    self.config,
                    output_root,
                    repository_root,
                    "token",
                )

            self.assertEqual(download_count, 1)
            self.assertEqual(
                (output_root / "build-1244644" / "image.iso").read_bytes(),
                self.payload,
            )
            self.assertEqual(
                (output_root / "build-1244644" / "update.cosi").read_bytes(),
                second_payload,
            )


if __name__ == "__main__":
    unittest.main()
