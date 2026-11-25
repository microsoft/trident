import importlib.util
import unittest

from pathlib import Path

MODULE_PATH = Path(__file__).resolve().parents[1] / "prepare_update_host_config.py"
SPEC = importlib.util.spec_from_file_location("prepare_update_host_config", MODULE_PATH)
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class PrepareUpdateHostConfigTests(unittest.TestCase):
    def test_prepare_host_config_versions_image_and_enables_self_upgrade(self):
        config = {
            "image": {
                "url": "http://192.0.2.1:8133/files/regular.cosi",
                "sha384": "old-hash",
            },
            "storage": {
                "disks": [{"id": "disk0", "deviceId": "scsi-0QEMU_QEMU_HARDDISK"}],
                "filesystems": [
                    {
                        "deviceId": "root",
                        "mountPoint": {"path": "/", "options": "defaults"},
                    }
                ],
            },
            "internalParams": {"existing": True},
            "scripts": {"preServicing": [{"name": "existing"}]},
        }

        update_url = MODULE.prepare_host_config(config, 2)

        self.assertEqual(
            update_url,
            "http://192.0.2.1:8133/files/regular_v2.cosi",
        )
        self.assertEqual(config["image"]["sha384"], "ignored")
        self.assertTrue(config["internalParams"]["selfUpgradeTrident"])
        self.assertTrue(config["internalParams"]["existing"])
        self.assertEqual(
            config["storage"],
            {
                "disks": [{"id": "disk0", "deviceId": "scsi-0QEMU_QEMU_HARDDISK"}],
                "filesystems": [
                    {
                        "deviceId": "root",
                        "mountPoint": {"path": "/", "options": "defaults"},
                    }
                ],
            },
        )
        self.assertEqual(
            config["scripts"]["preServicing"],
            [{"name": "existing"}],
        )

    def test_versioned_image_url_replaces_existing_version(self):
        self.assertEqual(
            MODULE.versioned_image_url(
                "https://example.test/files/regular_v1.cosi?token=value",
                2,
            ),
            "https://example.test/files/regular_v2.cosi?token=value",
        )

    def test_versioned_image_url_rejects_non_http_url(self):
        with self.assertRaisesRegex(ValueError, "Unsupported"):
            MODULE.versioned_image_url("oci://example.test/image:v1", 2)

    def test_prepare_host_config_requires_image_url(self):
        with self.assertRaisesRegex(ValueError, "image.url"):
            MODULE.prepare_host_config({"image": {}}, 2)

    def test_prepare_host_config_rejects_invalid_internal_params(self):
        config = {
            "image": {"url": "https://example.test/regular.cosi"},
            "internalParams": [],
        }

        with self.assertRaisesRegex(ValueError, "internalParams"):
            MODULE.prepare_host_config(config, 2)


if __name__ == "__main__":
    unittest.main()
