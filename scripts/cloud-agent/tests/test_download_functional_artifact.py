import importlib.util
import json
import tempfile
import unittest
import zipfile

from pathlib import Path

MODULE_PATH = Path(__file__).resolve().parents[1] / "download_functional_artifact.py"
SPEC = importlib.util.spec_from_file_location(
    "download_functional_artifact", MODULE_PATH
)
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class FunctionalArtifactTests(unittest.TestCase):
    def setUp(self):
        self.config = {
            "schemaVersion": 1,
            "definitionId": 5067,
            "buildId": 1244644,
            "sourceBranch": "refs/heads/main",
            "sourceVersion": "abc123",
            "buildResult": "succeeded",
            "artifactName": "trident-functest",
            "artifactType": "PipelineArtifact",
            "artifactSize": 10,
            "imageFilename": "trident-functest.qcow2",
        }

    def test_verify_build_rejects_source_mismatch(self):
        build = {
            "id": 1244644,
            "definition": {"id": 5067},
            "result": "succeeded",
            "sourceBranch": "refs/heads/main",
            "sourceVersion": "different",
        }

        with self.assertRaisesRegex(MODULE.ArtifactError, "sourceVersion"):
            MODULE.verify_build(self.config, build)

    def test_verify_artifact_returns_direct_download_url(self):
        artifact = {
            "name": "trident-functest",
            "resource": {
                "type": "PipelineArtifact",
                "properties": {"artifactsize": "10"},
                "downloadUrl": (
                    "https://example.artifacts.visualstudio.com/path/content"
                ),
            },
        }

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
        image = b"pinned image bytes"
        self.config["imageSize"] = len(image)
        self.config["imageSha256"] = MODULE.hashlib.sha256(image).hexdigest()

        with tempfile.TemporaryDirectory() as temp_dir:
            temp_path = Path(temp_dir)
            archive_path = temp_path / "artifact.zip"
            image_path = temp_path / "trident-functest.qcow2"
            with zipfile.ZipFile(archive_path, "w") as archive:
                archive.writestr("trident-functest/trident-functest.qcow2", image)

            MODULE.extract_image(self.config, archive_path, image_path)

            self.assertEqual(image_path.read_bytes(), image)
            self.assertTrue(MODULE.image_matches(self.config, image_path))

    def test_load_config_rejects_unknown_schema(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            config_path = Path(temp_dir) / "config.json"
            config_path.write_text(json.dumps({"schemaVersion": 2}), encoding="utf-8")

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

            self.assertTrue(link_path.is_symlink())
            self.assertEqual(link_path.resolve(), image_path.resolve())


if __name__ == "__main__":
    unittest.main()
