import argparse
import json
import subprocess

from pathlib import Path
from types import SimpleNamespace
from unittest.mock import MagicMock

import pytest

from . import conftest, test_setup


def make_request(options=None, cache=None):
    values = {} if cache is None else cache.copy()
    request = SimpleNamespace(
        config=SimpleNamespace(
            getoption=lambda key: (options or {}).get(key),
            cache=SimpleNamespace(get=values.get, set=values.__setitem__),
        )
    )
    return request, values


def namespace(request, reuse=True):
    image = conftest.functional_base_image.__wrapped__(request)
    return conftest.virtdeploy_namespace.__wrapped__(request, reuse, image)


def environment():
    return {
        conftest.VM_SSH_NODE_CACHE_KEY: "192.0.2.2",
        conftest.VM_NAMESPACE_CACHE_KEY: "functional-run",
        conftest.VM_NETWORK_CACHE_KEY: "192.0.2.0/24",
        conftest.VM_BASE_IMAGE_CACHE_KEY: str(Path("image.qcow2").resolve()),
    }


def test_options_preserve_existing_defaults():
    parser = argparse.ArgumentParser()
    conftest.pytest_addoption(SimpleNamespace(addoption=parser.add_argument))
    options = parser.parse_args([])
    assert not options.keep_environment
    assert not options.reuse_environment
    assert options.virtdeploy_namespace is None
    assert options.virtdeploy_network is None
    assert options.functional_base_image is None
    selected = parser.parse_args(
        [
            "--virtdeploy-namespace",
            "run",
            "--virtdeploy-network",
            "192.0.2.0/24",
            "--functional-base-image",
            "image.qcow2",
        ]
    )
    assert selected.virtdeploy_namespace == "run"
    assert selected.virtdeploy_network == "192.0.2.0/24"
    assert selected.functional_base_image == "image.qcow2"


@pytest.mark.parametrize("selected", [False, True])
def test_creation_options_cache_and_temporary_netlaunch(
    monkeypatch, tmp_path, selected
):
    options = (
        {
            "--virtdeploy-namespace": "functional-run",
            "--virtdeploy-network": "192.0.2.0/24",
            "--functional-base-image": "./image.qcow2",
        }
        if selected
        else {}
    )
    request, cache = make_request(options)
    image = conftest.functional_base_image.__wrapped__(request)
    ns = namespace(request, reuse=False)
    assert ns == ("functional-run" if selected else "virtdeploy")
    assert image == (
        Path("./image.qcow2").resolve() if selected else conftest.FT_BASE_IMAGE
    )
    default_netlaunch = tmp_path / "tools" / "vm-netlaunch.yaml"
    default_netlaunch.parent.mkdir()
    default_netlaunch.write_text("developer netlaunch")
    outputs = []

    def create(cmd, **kwargs):
        assert cmd[:3] == [tmp_path / "bin" / "virtdeploy", "create-one", "-J"]
        assert kwargs == {"capture_output": True, "text": True}
        assert cmd[cmd.index("--namespace") + 1] == ns
        assert cmd[cmd.index("--os-disk") + 1] == image
        assert cmd[cmd.index("-d") + 1] == "16,16"
        if selected:
            assert cmd[cmd.index("--network") + 1] == "192.0.2.0/24"
        else:
            assert "--network" not in cmd
        output = Path(cmd[cmd.index("--netlaunch") + 1])
        assert output.parent == Path(cmd[cmd.index("--ci-meta") + 1]).parent
        assert output.parent.is_dir()
        output.write_text("temporary netlaunch")
        outputs.append(output)
        return SimpleNamespace(
            stdout=json.dumps({"vms": [{"name": f"{ns}-vm-0", "ip": "192.0.2.2"}]})
        )

    monkeypatch.setattr(test_setup, "TRIDENT_REPO_DIR_PATH", tmp_path)
    monkeypatch.setattr(test_setup, "trident_runcmd", create)
    start = MagicMock()
    online = MagicMock()
    monkeypatch.setattr(test_setup.subprocess, "run", start)
    monkeypatch.setattr(test_setup, "wait_online", online)
    known_hosts = tmp_path / "known_hosts"
    test_setup.test_create_vm(request, known_hosts, "public-key", image, ns)
    start.assert_called_once_with(["virsh", "start", f"{ns}-vm-0"], check=True)
    online.assert_called_once_with("192.0.2.2", known_hosts, timeout=60)
    assert cache == {
        conftest.VM_SSH_NODE_CACHE_KEY: "192.0.2.2",
        conftest.VM_NAMESPACE_CACHE_KEY: ns,
        conftest.VM_NETWORK_CACHE_KEY: (
            "192.0.2.0/24" if selected else conftest.FT_NETWORK
        ),
        conftest.VM_BASE_IMAGE_CACHE_KEY: str(image),
    }
    assert namespace(request) == ns
    assert default_netlaunch.read_text() == "developer netlaunch"
    assert not outputs[0].exists()


@pytest.mark.parametrize("failure", ["create", "start", "online"])
def test_failed_creation_does_not_cache_success(monkeypatch, tmp_path, failure):
    request, cache = make_request(cache=environment())
    create = MagicMock(return_value={"name": "run-vm-0", "ip": "192.0.2.2"})
    start = MagicMock()
    online = MagicMock()
    failing_step = {"create": create, "start": start, "online": online}[failure]
    failing_step.side_effect = RuntimeError(failure)
    monkeypatch.setattr(test_setup, "create_vm", create)
    monkeypatch.setattr(test_setup.subprocess, "run", start)
    monkeypatch.setattr(test_setup, "wait_online", online)
    with pytest.raises(RuntimeError, match=failure):
        test_setup.test_create_vm(
            request, tmp_path / "known_hosts", "public-key", Path("image.qcow2"), "run"
        )
    assert all(value is None for value in cache.values())


def test_reuse_skips_creation_without_changing_cache(monkeypatch):
    request, cache = make_request({"--reuse-environment": True}, environment())
    create = MagicMock()
    monkeypatch.setattr(test_setup, "create_vm", create)
    test_setup.test_create_vm(
        request,
        Path("known_hosts"),
        "public-key",
        Path("image.qcow2"),
        namespace(request),
    )
    create.assert_not_called()
    assert cache == environment()


@pytest.mark.parametrize(
    "option,key,matching",
    [
        ("--virtdeploy-namespace", conftest.VM_NAMESPACE_CACHE_KEY, "functional-run"),
        ("--virtdeploy-network", conftest.VM_NETWORK_CACHE_KEY, "192.0.2.0/24"),
        ("--functional-base-image", conftest.VM_BASE_IMAGE_CACHE_KEY, "./image.qcow2"),
    ],
)
def test_reuse_validates_explicit_selection(option, key, matching):
    request, _ = make_request({option: matching}, environment())
    assert namespace(request) == "functional-run"
    for metadata in (environment(), {}):
        metadata[key] = "different"
        request, _ = make_request({option: matching}, metadata)
        with pytest.raises(pytest.fail.Exception, match=option):
            namespace(request)
    metadata = environment()
    del metadata[key]
    request, _ = make_request({option: matching}, metadata)
    with pytest.raises(pytest.fail.Exception, match="metadata is missing"):
        namespace(request)


def test_legacy_default_reuse_is_visible_and_compatible(caplog):
    request, _ = make_request(cache={conftest.VM_SSH_NODE_CACHE_KEY: "192.0.2.2"})
    assert namespace(request) == "virtdeploy"
    assert "Legacy VM cache has no namespace metadata" in caplog.text


def test_reuse_requires_namespace_even_when_explicit_network_matches():
    metadata = environment()
    del metadata[conftest.VM_NAMESPACE_CACHE_KEY]
    request, _ = make_request({"--virtdeploy-network": "192.0.2.0/24"}, metadata)
    with pytest.raises(pytest.fail.Exception, match="No cached VM namespace"):
        namespace(request)


@pytest.mark.parametrize(
    "keep,fail_cleanup", [(False, False), (False, True), (True, False)]
)
def test_teardown_restores_namespace_and_retains_cache_on_failure_or_keep(
    monkeypatch, keep, fail_cleanup
):
    request, cache = make_request({"--keep-environment": keep}, environment())
    node = MagicMock()
    coverage = MagicMock()
    run = MagicMock()
    if fail_cleanup:
        run.side_effect = subprocess.CalledProcessError(1, "virtdeploy clean")
    monkeypatch.setattr(conftest, "SshNode", node)
    monkeypatch.setattr(conftest, "fetch_code_coverage", coverage)
    monkeypatch.setattr(conftest, "trident_runcmd", run)
    fixture = conftest.vm.__wrapped__(
        request, Path("id_rsa.pub"), Path("known_hosts"), namespace(request)
    )
    assert next(fixture) is node.return_value
    node.assert_called_once_with(
        ".",
        "log",
        "192.0.2.2",
        username=conftest.TEST_USER,
        key_path=Path("id_rsa"),
        known_hosts_path=Path("known_hosts"),
    )
    with pytest.raises(
        subprocess.CalledProcessError if fail_cleanup else StopIteration
    ):
        next(fixture)
    coverage.assert_called_once_with(node.return_value)
    if keep:
        run.assert_not_called()
    else:
        run.assert_called_once_with(
            [
                conftest.TRIDENT_REPO_DIR_PATH / "bin" / "virtdeploy",
                "clean",
                "--namespace",
                "functional-run",
            ]
        )
    assert cache == (
        environment() if keep or fail_cleanup else dict.fromkeys(environment())
    )


def test_coverage_download_creates_output_directory(monkeypatch, tmp_path):
    node = MagicMock()
    node.ssh_client.open_sftp.return_value.__enter__.return_value.listdir.return_value = [
        "test.profraw",
        "not-coverage.txt",
    ]
    node.copy_back.side_effect = lambda source, destination: destination.write_bytes(
        b"coverage"
    )
    monkeypatch.setattr(conftest, "TRIDENT_REPO_DIR_PATH", tmp_path)
    conftest.fetch_code_coverage(node)
    destination = tmp_path / "target" / "coverage" / "profraw" / "test.profraw"
    node.copy_back.assert_called_once_with("test.profraw", destination)
    assert destination.read_bytes() == b"coverage"
