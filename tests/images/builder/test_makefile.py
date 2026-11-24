from pathlib import Path
import shlex
import subprocess
import sys
import unittest


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
