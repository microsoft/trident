import base64
import importlib.util
import tempfile
import unittest

from pathlib import Path

MODULE_PATH = Path(__file__).resolve().parents[1] / "prepare_install_host_config.py"
SPEC = importlib.util.spec_from_file_location(
    "prepare_install_host_config", MODULE_PATH
)
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class PrepareInstallHostConfigTests(unittest.TestCase):
    def write_policy(self, root: Path) -> dict[str, bytes]:
        contents = {
            "trident.te": b"policy_module(trident, 1.0)\n",
            "trident.fc": b"/usr/bin/trident -- gen_context(system_u:object_r:trident_exec_t,s0)\n",
        }
        for name, content in contents.items():
            (root / name).write_bytes(content)
        return contents

    def test_prepare_host_config_enables_self_upgrade_and_preserves_scripts(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            policy_dir = Path(temp_dir)
            self.write_policy(policy_dir)
            config = {
                "scripts": {
                    "preServicing": [
                        {
                            "name": "existing",
                            "runOn": ["clean-install"],
                            "content": "true",
                        }
                    ]
                }
            }

            MODULE.prepare_host_config(config, policy_dir)

            self.assertTrue(config["internalParams"]["selfUpgradeTrident"])
            scripts = config["scripts"]["preServicing"]
            self.assertEqual(scripts[0]["name"], MODULE.POLICY_SCRIPT_NAME)
            self.assertEqual(scripts[1]["name"], "existing")

    def test_policy_loader_embeds_candidate_policy_files(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            policy_dir = Path(temp_dir)
            contents = self.write_policy(policy_dir)

            loader = MODULE.build_policy_loader(policy_dir)

            for content in contents.values():
                encoded = base64.encodebytes(content).decode("ascii").rstrip()
                self.assertIn(encoded, loader)
            self.assertIn("make -C", loader)
            self.assertIn("semodule -i", loader)

    def test_policy_loader_rejects_missing_files(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            with self.assertRaisesRegex(ValueError, "trident.te"):
                MODULE.build_policy_loader(Path(temp_dir))


if __name__ == "__main__":
    unittest.main()
