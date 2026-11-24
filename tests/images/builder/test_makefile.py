from pathlib import Path
import subprocess
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
