import lzma
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from builder import BaseImageManifest
from builder.download import download_base_image


class DownloadBaseImageTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.target = Path(directory.name) / "artifacts" / "base.vhdx"
        self.image = BaseImageManifest(
            image=SimpleNamespace(name="azl4_qemu_guest", path=self.target),
            package_name="minimal-os",
            version="4",
            catalog_reference="mcr.microsoft.com/azurelinux/catalog/minimal-os:4",
        )

    def pull(self, files):
        def run(command, **kwargs):
            destination = Path(command[command.index("--output") + 1])
            for name, contents in files.items():
                (destination / name).write_bytes(contents)

        return patch("builder.download.subprocess.run", side_effect=run)

    def test_azl4_catalog_download_expands_xz(self):
        with self.pull({"image.vhdx.xz": lzma.compress(b"disk image")}) as pull:
            download_base_image(self.image)

        self.assertEqual(self.target.read_bytes(), b"disk image")
        self.assertEqual(
            pull.call_args.args[0][2],
            "mcr.microsoft.com/azurelinux/catalog/minimal-os:4",
        )

    def test_azl3_download_keeps_existing_reference_and_plain_vhdx(self):
        self.image.catalog_reference = None
        self.image.image.name = "baremetal"
        with self.pull({"image.vhdx": b"plain image"}) as pull:
            download_base_image(self.image)

        self.assertEqual(self.target.read_bytes(), b"plain image")
        self.assertEqual(
            pull.call_args.args[0][2],
            "mcr.microsoft.com/azurelinux/3.0/image/baremetal:latest",
        )

    def test_invalid_xz_does_not_replace_existing_image(self):
        self.target.parent.mkdir()
        self.target.write_bytes(b"existing image")
        with self.pull({"image.vhdx.xz": b"invalid archive"}):
            with self.assertRaises(lzma.LZMAError):
                download_base_image(self.image)

        self.assertEqual(self.target.read_bytes(), b"existing image")
        self.assertEqual(list(self.target.parent.iterdir()), [self.target])

    def test_rejects_ambiguous_plain_and_compressed_images(self):
        with self.pull({"a.vhdx": b"first", "b.vhdx.xz": lzma.compress(b"second")}):
            with self.assertRaisesRegex(RuntimeError, "Multiple .vhdx"):
                download_base_image(self.image)

        self.assertFalse(self.target.exists())
