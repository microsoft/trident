from pathlib import Path
import subprocess
import unittest


class StripSelinuxXattrsTests(unittest.TestCase):
    def run_check(self, scan):
        script = (
            Path(__file__).resolve().parents[1]
            / "trident-testimage/base/scripts/strip-selinux-xattrs.sh"
        ).read_text()
        return subprocess.run(
            [
                "bash",
                "-c",
                f"find() {{ :; }}\ngetfattr() {{ {scan}; }}\n{script}",
            ],
            capture_output=True,
            text=True,
            timeout=10,
        )

    def test_rejects_labels_in_large_scan(self):
        result = self.run_check("printf '%s\\n' security.selinux; seq 100000")

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("ERROR: security.selinux xattr still present", result.stderr)

    def test_accepts_scan_without_labels(self):
        result = self.run_check(":")

        self.assertEqual(result.returncode, 0, result.stderr)
