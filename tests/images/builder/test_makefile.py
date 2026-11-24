from contextlib import redirect_stdout
import io
from pathlib import Path
import shlex
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from builder import ImageConfig, OutputFormat, run


class MakefileTests(unittest.TestCase):
    def test_azl4_base_uses_catalog_download_rule(self):
        repo = Path(__file__).resolve().parents[3]
        result = subprocess.run(
            [
                "make",
                "--no-print-directory",
                "-n",
                "-B",
                "artifacts/azl4_qemu_guest.vhdx",
            ],
            cwd=repo,
            capture_output=True,
            text=True,
            check=True,
        )

        self.assertIn(
            "tests/images/testimages.py download-image azl4_qemu_guest", result.stdout
        )
        self.assertNotIn("testimages.py build", result.stdout)
        self.assertNotIn("is not defined", result.stderr)

    def dry_run(self, *targets):
        repo = Path(__file__).resolve().parents[3]
        prerequisites = {
            "artifacts/id_rsa",
            "artifacts/id_rsa.pub",
            "artifacts/rpm-overrides",
            "tests/images/trident-vm-testimage/base/files/id_rsa.pub",
        }
        for target in targets:
            dependencies = subprocess.check_output(
                [
                    sys.executable,
                    "tests/images/testimages.py",
                    "dependencies",
                    Path(target).stem,
                ],
                cwd=repo,
                text=True,
            )
            prerequisites.update(shlex.split(dependencies))
        return subprocess.check_output(
            [
                "make",
                "--no-print-directory",
                "-n",
                "-B",
                *(f"--assume-old={path}" for path in sorted(prerequisites)),
                *targets,
            ],
            cwd=repo,
            text=True,
            stderr=subprocess.STDOUT,
        )

    def test_azl3_vm_targets_use_original_customizer_path(self):
        for image, output_format in (
            ("trident-vm-grub-testimage.qcow2", "qcow2"),
            ("trident-vm-grub-verity-azure-testimage.vhd", "vhd-fixed"),
        ):
            with self.subTest(image=image):
                output = self.dry_run(f"artifacts/{image}")

                self.assertIn("docker run --rm", output)
                self.assertIn(f"--output-image-format {output_format}", output)
                self.assertIn("--rpm-source /repo/artifacts/rpm-overrides", output)
                self.assertNotIn("testimages.py build", output)

    def test_azl3_cosi_keeps_default_builder_format(self):
        output = self.dry_run("artifacts/trident-vm-grub-testimage.cosi")

        self.assertIn("testimages.py build", output)
        self.assertNotIn("--output-type", output)

    def test_azl4_formats_are_built_independently(self):
        output = self.dry_run(
            "artifacts/trident-vm-grub-azl4-testimage.cosi",
            "artifacts/trident-vm-grub-azl4-testimage.qcow2",
        )

        self.assertEqual(output.count("testimages.py build"), 2)
        self.assertIn("--output-type cosi", output)
        self.assertIn("--output-type qcow2", output)


class OutputSelectionTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        (root / "image").mkdir()
        (root / "base.vhdx").touch()
        (root / "image" / "update.yaml").write_text("recipe: update\n")
        (root / "image" / "base.yaml").write_text("recipe: base\n")
        self.image = ImageConfig(
            "image",
            source=str(root),
            base_image=SimpleNamespace(path=root / "base.vhdx"),
            config_file="update.yaml",
            requires_trident=False,
            output_configs={OutputFormat.QCOW2: "base.yaml"},
        )
        self.output_dir = root / "artifacts"

    def build(self, output_type=None):
        with patch("builder.run.build_image") as build_image:
            run.build(
                artifacts=None,
                configs=[self.image],
                name=self.image.name,
                container_name="imagecustomizer:test",
                output_dir=self.output_dir,
                clones=1,
                dry_run=True,
                force=False,
                output_type=output_type,
            )
        return build_image.call_args.kwargs["image"]

    def test_selects_recipe_without_mutating_default_or_reusing_yaml_cache(self):
        self.assertEqual(self.image.base_ic_config["recipe"], "update")
        selected = self.build("qcow2")
        self.assertEqual(selected.file_name(), "image.qcow2")
        self.assertEqual(selected.base_ic_config["recipe"], "base")
        for output_type in (None, "cosi"):
            default = self.build(output_type)
            self.assertEqual(default.file_name(), "image.cosi")
            self.assertEqual(default.base_ic_config["recipe"], "update")
        self.assertEqual(self.image.config_file, Path("update.yaml"))
        self.assertEqual(self.image.output_format, OutputFormat.COSI)

    def test_rejects_unsupported_format(self):
        with self.assertRaisesRegex(ValueError, "not supported"):
            self.build("vhd")

    def test_legacy_same_extension_format_remains_unchanged(self):
        self.image.output_configs = {}
        self.image.output_format = OutputFormat.BAREMETAL_IMAGE
        self.assertEqual(self.build().output_format, OutputFormat.BAREMETAL_IMAGE)
        with self.assertRaisesRegex(ValueError, "not supported"):
            self.build("cosi")

    def test_list_filters_include_alternate_format_once(self):
        for filter_type in (None, "cosi", "qcow2", "vhd"):
            with self.subTest(filter_type=filter_type):
                output = io.StringIO()
                with redirect_stdout(output):
                    run.list_configs(configs=[self.image], filter_type=filter_type)
                self.assertEqual(
                    output.getvalue(), "" if filter_type == "vhd" else "image\n"
                )
