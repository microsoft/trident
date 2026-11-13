import tempfile
import unittest

from pathlib import Path
from types import SimpleNamespace
from unittest.mock import MagicMock, patch

import pytest

from . import conftest, test_setup


class FunctionalFixtureTests(unittest.TestCase):
    def request(self, options, cache):
        return SimpleNamespace(
            config=SimpleNamespace(
                getoption=lambda key: options.get(key),
                cache=SimpleNamespace(get=lambda key, default: cache.get(key, default)),
            )
        )

    def test_reuse_restores_namespace_and_rejects_mismatch_or_missing_metadata(self):
        request = self.request({}, {conftest.VM_NAMESPACE_CACHE_KEY: "functional-run"})
        self.assertEqual(
            conftest.virtdeploy_namespace.__wrapped__(request, True), "functional-run"
        )
        for options, cache in (
            (
                {"--virtdeploy-namespace": "different"},
                {conftest.VM_NAMESPACE_CACHE_KEY: "functional-run"},
            ),
            ({}, {}),
        ):
            with self.subTest(options=options, cache=cache):
                with self.assertRaises(pytest.fail.Exception):
                    conftest.virtdeploy_namespace.__wrapped__(
                        self.request(options, cache), True
                    )

    def test_creation_caches_namespace_with_address(self):
        request = MagicMock()
        request.config.getoption.side_effect = lambda key: (
            False if key == "--reuse-environment" else None
        )
        with tempfile.TemporaryDirectory() as temp_dir, patch.object(
            test_setup,
            "create_vm",
            return_value={"name": "run-vm-0", "ip": "192.0.2.2"},
        ), patch.object(test_setup.subprocess, "run"), patch.object(
            test_setup, "wait_online"
        ):
            test_setup.test_create_vm(
                request,
                Path(temp_dir) / "known_hosts",
                "ssh-public-key",
                Path(temp_dir) / "image.qcow2",
                "run",
            )
        request.config.cache.set.assert_any_call(conftest.VM_NAMESPACE_CACHE_KEY, "run")
        request.config.cache.set.assert_any_call(
            conftest.VM_SSH_NODE_CACHE_KEY, "192.0.2.2"
        )

    def test_teardown_uses_restored_namespace(self):
        request = MagicMock()
        request.config.getoption.return_value = None
        request.config.cache.get.return_value = "192.0.2.2"
        with patch.object(conftest, "SshNode"), patch.object(
            conftest, "fetch_code_coverage"
        ), patch.object(conftest, "trident_runcmd") as run:
            fixture = conftest.vm.__wrapped__(
                request, Path("id_rsa.pub"), Path("known_hosts"), "functional-run"
            )
            next(fixture)
            with self.assertRaises(StopIteration):
                next(fixture)
        self.assertEqual(run.call_args.args[0][-2:], ["--namespace", "functional-run"])

    def test_relative_base_image_resolves_from_caller_directory(self):
        request = self.request({"--functional-base-image": "./image.qcow2"}, {})
        self.assertEqual(
            conftest.functional_base_image.__wrapped__(request),
            Path("./image.qcow2").resolve(),
        )

    def test_coverage_download_creates_missing_output_directory(self):
        ssh_node = MagicMock()
        ssh_node.ssh_client.open_sftp.return_value.__enter__.return_value.listdir.return_value = [
            "test.profraw"
        ]

        def copy_back(source, destination):
            destination.write_bytes(b"coverage")

        ssh_node.copy_back.side_effect = copy_back
        with tempfile.TemporaryDirectory() as temp_dir, patch.object(
            conftest, "TRIDENT_REPO_DIR_PATH", Path(temp_dir)
        ):
            conftest.fetch_code_coverage(ssh_node)
            self.assertEqual(
                (Path(temp_dir) / "target/coverage/profraw/test.profraw").read_bytes(),
                b"coverage",
            )
