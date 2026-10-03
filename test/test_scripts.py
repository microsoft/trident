import importlib.util
from pathlib import Path
import unittest

HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("installer_vm", HERE / "run.py")
VM = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(VM)


class VmCommandTests(unittest.TestCase):
    def test_vnc_is_headless_and_bound_to_loopback(self):
        launch = VM.command(
            Path("/tmp/installer-test.iso"),
            Path("/tmp/installer-test-owned"),
            Path("/usr/share/OVMF/OVMF_CODE_4M.fd"),
            6144,
            2,
            "gtk",
            vnc=1,
        )
        self.assertEqual(launch[launch.index("-display") + 1], "none")
        self.assertEqual(launch[launch.index("-vnc") + 1], "127.0.0.1:1")
        self.assertNotIn("gtk", launch)

    def test_vm_exposes_only_new_disposable_storage(self):
        scratch = Path("/tmp/installer-test-owned")
        launch = VM.command(
            Path("/tmp/installer-test.iso"),
            scratch,
            Path("/usr/share/OVMF/OVMF_CODE_4M.fd"),
            6144,
            2,
            "gtk",
        )
        drives = [
            launch[index + 1] for index, value in enumerate(launch) if value == "-drive"
        ]
        self.assertEqual(len(drives), 4)
        self.assertIn(f"file={scratch}/disk.qcow2", drives[2])
        self.assertIn("readonly=on", drives[0])
        self.assertIn("readonly=on", drives[3])
        self.assertNotIn("-virtfs", launch)
        self.assertNotIn("-fsdev", launch)
        self.assertIn("virtio-blk-pci,drive=hd0,bootindex=1", launch)
        self.assertIn("ide-cd,bus=ide.0,drive=cd0,bootindex=2", launch)
        self.assertIn("gtk", launch)
        self.assertNotIn("-vnc", launch)
        self.assertEqual(
            launch[launch.index("-serial") + 1], f"file:{scratch}/serial.log"
        )


if __name__ == "__main__":
    unittest.main()
