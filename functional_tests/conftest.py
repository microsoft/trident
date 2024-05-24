# pytest will expose fixtures in conftest.py to sibling files.

import os
import pytest
import subprocess
import logging
import tempfile
import fnmatch
import json

from functools import partial
from typing import Any, Dict, Iterable, List, Optional, Union
from pytest import Collector, File, Function, Item
from pathlib import Path

from .ssh_node import SshNode

# Load class dependency plug-in
pytest_plugins = ["functional_tests.depends"]

"""Location of the trident repository."""
TRIDENT_REPO_DIR_PATH = Path(__file__).resolve().parent.parent

NETLAUNCH_BIN_REL_PATH = Path("bin/netlaunch")

NETLAUNCH_BIN_PATH = TRIDENT_REPO_DIR_PATH / NETLAUNCH_BIN_REL_PATH


def __get_argus_toolkit_path():
    """Returns the path to the argus-toolkit repository."""
    envvar = os.environ.get("ARGUS_TOOLKIT_PATH", None)
    if envvar:
        return Path(envvar).resolve()
    return Path(__file__).resolve().parent.parent.parent / "argus-toolkit"


"""Location of the argus-toolkit repository."""
ARGUS_REPO_DIR_PATH = __get_argus_toolkit_path()

"""The user to use for SSH connections to the VM.
Needs to be in sync with the
user specified in the trident-setup.yaml."""
TEST_USER = "testuser"

"""The name of the file containing the remote address of the VM."""
REMOTE_ADDR_FILENAME = "remote-addr"

"""The name of the file containing the known hosts for SSH connections."""
KNOWN_HOSTS_FILENAME = "known_hosts"

VM_SSH_NODE_CACHE_KEY = "vm_ssh_node"


def __get_installer_iso_path():
    """Returns the path to the installer ISO."""
    envvar = os.environ.get("INSTALLER_ISO_PATH", None)
    if envvar:
        return Path(envvar).resolve()
    return TRIDENT_REPO_DIR_PATH / "bin" / "trident-mos.iso"


"""Location of the installer ISO.
Defined in the makefile.
"""
INSTALLER_ISO_PATH = __get_installer_iso_path()

"""Location of the directory netlaunch will serve from"""
NETLAUNCH_SERVE_DIRECTORY = TRIDENT_REPO_DIR_PATH / "artifacts" / "test-image"


def pytest_addoption(parser):
    """Defines additional command line options for the tests."""
    parser.addoption(
        "--keep-environment",
        action="store_true",
        help="Keep VM environment after tests complete.",
    )

    parser.addoption(
        "--reuse-environment",
        action="store_true",
        help="Reuse VM environment from previous tests.",
    )

    parser.addoption("--test-dir", action="store", help="Location to store test files.")

    parser.addoption(
        "--ssh-key",
        action="store",
        help="SSH key to use for connecting to VM.",
        default=os.path.expanduser("~/.ssh/id_rsa.pub"),
    )

    parser.addoption(
        "--build-output",
        action="store",
        help="Path to the JSON formatted build output.",
    )

    parser.addoption(
        "--force-upload",
        action="store_true",
        help="Force upload of tests even if no change was detected.",
    )

    parser.addoption(
        "--redeploy", action="store_true", help="Redeploy OS using Trident."
    )


def pytest_collect_file(file_path: Path, parent: Collector) -> Optional[Collector]:
    """Creates a custom collector for ft.json."""
    if file_path.name == "ft.json":
        # Note: name is ignored here, but is needed by the constructor.
        return FuncTestCollector.from_parent(parent, name="functest", path=file_path)


class FuncTestCollector(File):
    """A custom collector for Functional tests defined in ft.json.
    `ft.json` is the output of the pytest crate, produced by running
    `trident pytest`
    """

    def __init__(self, **kwargs):
        super().__init__(**kwargs)

    def collect(self) -> Iterable[Union[Item, Collector]]:
        with open(self.path, "r") as f:
            import json

            data = json.load(f)
        for crate, crate_data in data.items():
            yield RustModule.from_parent(
                self, name=crate, crate=crate, module_data=crate_data
            )


class RustModule(Collector):
    """A custom collector for Rust modules."""

    def __init__(
        self,
        crate: str,
        module_data: Dict[str, Dict[str, Any]],
        module_path: List[str] = [],
        **kwargs,
    ):
        self.crate = crate
        self.module_data = module_data
        self.module_path = module_path
        super().__init__(**kwargs)

    def collect(self) -> Iterable[Union[Item, Collector]]:
        # Yield a new collector for each submodule
        for module_name, module_data in self.module_data.get("submodules", {}).items():
            yield RustModule.from_parent(
                self,
                crate=self.crate,
                name=module_name,
                module_data=module_data,
                module_path=self.module_path + [module_name],
            )

        # Yield a function for each test case
        for test_name, test_data in self.module_data.get("test_cases", {}).items():
            node = Function.from_parent(
                self,
                name=test_name,
                callobj=partial(
                    run_rust_functional_test,
                    crate=self.crate,
                    module_path="::".join(self.module_path),
                    test_case=test_name,
                ),
            )
            for marker in test_data.get("markers", []):
                node.add_marker(marker)
            yield node


@pytest.mark.depends("test_deploy_vm")
def run_rust_functional_test(vm, wipe_sdb, crate, module_path, test_case):
    """Runs a rust test on the VM."""
    from functional_tests.tools.runner import RunnerTool

    testRunner = RunnerTool(vm)
    testRunner.run(
        crate,
        f"{module_path}::{test_case}",
    )


@pytest.fixture(scope="function")
def wipe_sdb(vm: SshNode):
    """Wipes the SDB on the VM."""
    vm.execute("sudo wipefs -af /dev/sdb")
    yield
    vm.execute("sudo wipefs -af /dev/sdb")


def fetch_code_coverage(ssh_node):
    """Downloads all code coverage files from the VM."""
    ssh_node.execute("sudo chown -R {} .".format(TEST_USER))
    with ssh_node.ssh_client.open_sftp() as sftp:
        for filename in sftp.listdir("."):
            if fnmatch.fnmatch(filename, "*.profraw"):
                ssh_node.copy_back(
                    filename,
                    TRIDENT_REPO_DIR_PATH
                    / "target"
                    / "coverage"
                    / "profraw"
                    / filename,
                )
    ssh_node.execute("find . -name '*.profraw' -delete")


def trident_runcmd(cmd, check=True, **kwargs):
    """Runs a command in the trident repository directory."""
    logging.debug(f"Running command: {cmd}")
    subprocess.run(cmd, check=check, cwd=TRIDENT_REPO_DIR_PATH, **kwargs)


def argus_runcmd(cmd, check=True, **kwargs):
    """Runs a command in the argus repository directory."""
    logging.debug(f"Running command: {cmd}")
    subprocess.run(cmd, check=check, cwd=ARGUS_REPO_DIR_PATH, **kwargs)


def upload_test_binaries(build_output_path: Path, force_upload, ssh_node):
    """Uploads all test binaries to the VM. Unless force_upload is set, only binaries
    that are not fresh are uploaded. You need to make sure that you dont rebuild
    the test binaries between the build and the upload, as the freshness is
    indicated by the cargo build output.
    """
    ssh_node.execute("mkdir -p tests")
    for line in open(build_output_path):
        report = json.loads(line)
        if (
            "target" in report
            and "kind" in report["target"]
            and "lib" in report["target"]["kind"]
            and "executable" in report
            and report["executable"]
        ):
            if force_upload or not report["fresh"]:
                test_binary = report["executable"]
                filename = os.path.basename(test_binary)
                stripped_name = filename.split("-", 2)[0]
                ssh_node.copy(test_binary, "tests/{}".format(stripped_name))
                ssh_node.execute("chmod +x tests/{}".format(stripped_name))


@pytest.fixture(scope="session")
def ssh_key_path(request) -> Path:
    return Path(request.config.getoption("--ssh-key"))


@pytest.fixture(scope="session")
def ssh_key_public(request) -> Path:
    with open(request.config.getoption("--ssh-key"), "r") as f:
        return f.read().strip()


@pytest.fixture(scope="session")
def redeploy(request) -> bool:
    return bool(request.config.getoption("--redeploy"))


@pytest.fixture(scope="session")
def reuse_environment(request) -> bool:
    return bool(request.config.getoption("--reuse-environment"))


@pytest.fixture(scope="session")
def test_dir_path(request, reuse_environment) -> Optional[Path]:
    test_dir_path = request.config.getoption("--test-dir", None)
    test_dir_path = Path(test_dir_path) if test_dir_path else None

    if reuse_environment:
        if not test_dir_path or not test_dir_path.is_dir():
            pytest.skip(
                "Must specify --test-dir pointing to an existing test directory when using --reuse-environment"
            )
        yield Path(test_dir_path)
    else:
        # Sets up the test directory. If test_dir_path is None, a temporary
        # directory is created instead. Note that if you want to reuse the test
        # environment, it is beneficial to pass a specific directory.
        if test_dir_path:
            if test_dir_path.is_dir():
                # Throw away known hosts from the last time.
                known_hosts_file = Path(test_dir_path) / KNOWN_HOSTS_FILENAME
                if known_hosts_file.exists():
                    known_hosts_file.unlink()
            else:
                test_dir_path.mkdir(parents=True, exist_ok=True)
            yield test_dir_path
        else:
            with tempfile.TemporaryDirectory() as temp_dir:
                yield Path(temp_dir)


@pytest.fixture(scope="session")
def remote_addr_path(test_dir_path) -> Path:
    return test_dir_path / REMOTE_ADDR_FILENAME


@pytest.fixture(scope="session")
def known_hosts_path(test_dir_path, reuse_environment, redeploy) -> Path:
    kh = test_dir_path / KNOWN_HOSTS_FILENAME
    if reuse_environment and not redeploy:
        if not kh.is_file():
            pytest.fail(
                "No known hosts file found in test directory. You might need to recreate the test environment using make functional-test"
            )
    return kh


@pytest.fixture(scope="session")
def vm(request, ssh_key_path, known_hosts_path) -> SshNode:
    """Define the VirtDeploy based LibVirt VM as a resource the tests can use."""
    build_output = request.config.getoption("--build-output")
    force_upload = request.config.getoption("--force-upload")
    keep_environment = request.config.getoption("--keep-environment")

    ssh_node_address = request.config.cache.get(VM_SSH_NODE_CACHE_KEY, None)
    if ssh_node_address is None:
        pytest.skip("VM not setup!")

    priv_key = ssh_key_path.with_suffix("")
    logging.info(f"Using SSH key {priv_key}")

    ssh_node = SshNode(
        ".",
        "log",
        ssh_node_address,
        username=TEST_USER,
        key_path=priv_key,
        known_hosts_path=known_hosts_path,
    )

    if build_output:
        upload_test_binaries(build_output, force_upload, ssh_node)

    # Setup complete.
    yield ssh_node

    # Save the code coverage files, so we can track the overall code coverage.
    fetch_code_coverage(ssh_node)

    if keep_environment:
        # Skip fixture cleanup.
        return

    request.config.cache.set(VM_SSH_NODE_CACHE_KEY, None)

    # Fixture cleanup.
    argus_runcmd([ARGUS_REPO_DIR_PATH / "virt-deploy", "create", "--clean"])


def pytest_collection_modifyitems(session, config, items: List[pytest.Item]):
    """Artificially force the setup tests to run first."""

    # Get all the setup items.
    setup_items = [item for item in items if "test_setup.py" in item.nodeid]

    # Before we do anything, we need to remove the setup items from the list.
    for item in setup_items:
        items.remove(item)

    # Because of how pytest is invoked, we may have duplicate setup items.
    # We should only keep one of each. First we make a set of the nodeids.
    unique_nodeids = set([item.nodeid for item in setup_items])

    # Filter out the unique setup items.
    def is_unique(item):
        """Returns True if the item hasn't been checked, and removes it from the
        unique items set, False otherwise."""
        if item.nodeid in unique_nodeids:
            unique_nodeids.remove(item.nodeid)
            return True
        return False

    setup_items = list(filter(is_unique, setup_items))

    # Reverse to push them to the front of the list in the order they were added.
    setup_items.reverse()

    # Remove the setup items from the list and add them back in the front.
    for item in setup_items:
        items.insert(0, item)
