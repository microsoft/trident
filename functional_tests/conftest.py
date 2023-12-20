# pytest will expose fixtures in conftest.py to sibling files.

import os
import pytest
import subprocess
import logging
import yaml
import tempfile
import fnmatch
import json

from pathlib import Path

from .ssh_node import SshNode

"""Location of the trident repository."""
TRIDENT_REPO_DIR_PATH = Path(__file__).resolve().parent.parent

"""Location of the argus-toolkit repository."""
ARGUS_REPO_DIR_PATH = Path(__file__).resolve().parent.parent.parent / "argus-toolkit"

"""The user to use for SSH connections to the VM.
Needs to be in sync with the
user specified in the trident-setup.yaml."""
TEST_USER = "testuser"

"""The name of the file containing the remote address of the VM."""
REMOTE_ADDR_FILENAME = "remote-addr"

"""The name of the file containing the known hosts for SSH connections."""
KNOWN_HOSTS_FILENAME = "known_hosts"


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


def disable_phonehome(ssh_node: SshNode):
    """Disables phonehome in the VM to allow faster rerunning of Trident."""
    ssh_node.execute("sudo sed -i 's/phonehome: .*//' /etc/trident/config.yaml")


def deploy_os(host_config_path: Path, remote_addr_path: Path, installer_iso_path: Path):
    """Deploys the OS to the VM using netlaunch."""
    argus_runcmd(
        [
            ARGUS_REPO_DIR_PATH / "build" / "netlaunch",
            "-i",
            installer_iso_path,
            "-c",
            "vm-netlaunch.yaml",
            "-t",
            host_config_path,
            "-l",
            "-r",
            remote_addr_path,
        ]
    )


def setup_testdir(test_dir_path: Path):
    """Sets up the test directory. If test_dir_path is None, a temporary directory is
    created instead. Note that if you want to reuse the test environment, it is
    beneficial to pass a specific directory.
    """
    if test_dir_path:
        test_dir_path = Path(test_dir_path)
        if test_dir_path.is_dir():
            # Throw away known hosts from the last time.
            known_hosts_file = Path(test_dir_path) / KNOWN_HOSTS_FILENAME
            if known_hosts_file.exists():
                known_hosts_file.unlink()
        else:
            test_dir_path.mkdir(parents=True, exist_ok=True)
    else:
        test_dir_path = tempfile.TemporaryDirectory()
    return test_dir_path


def prepare_hostconfig(test_dir_path: Path, ssh_key_path: Path):
    """Sets up the host configuration file for the VM."""
    # Read user's public key from ~/.ssh/id_rsa.pub
    with open(ssh_key_path, "r") as file:
        pubkey = file.read().strip()

    # Add user's public key to trident-setup.yaml
    with open(
        TRIDENT_REPO_DIR_PATH / "functional_tests/trident-setup.yaml", "r"
    ) as file:
        trident_setup = yaml.safe_load(file)
    trident_setup["hostConfiguration"]["osconfig"]["users"][0]["sshKeys"].clear()
    trident_setup["hostConfiguration"]["osconfig"]["users"][0]["sshKeys"].append(pubkey)

    prepped_host_config_path = test_dir_path / "trident-setup.yaml"
    with open(prepped_host_config_path, "w") as file:
        yaml.dump(trident_setup, file)

    return prepped_host_config_path


def create_vm(create_params):
    """Creates a VM with the given parameters, using virt-deploy."""
    argus_runcmd([ARGUS_REPO_DIR_PATH / "virt-deploy", "create"] + create_params)
    argus_runcmd([ARGUS_REPO_DIR_PATH / "virt-deploy", "run"])


def inject_ssh_key(remote_addr_path: Path, known_hosts_path: Path):
    """# Temporary solution to initialize the known_hosts file until we can inject a
    predictable key.
    """
    with open(remote_addr_path, "r") as file:
        remote_addr = file.read().strip()
    with open(known_hosts_path, "w") as file:
        subprocess.run(["ssh-keyscan", remote_addr], stdout=file, check=True)


def deploy_vm(
    test_dir_path: Path,
    ssh_key_path: Path,
    known_hosts_path: Path,
    installer_iso_path: Path,
):
    """# Provision a VM with the given parameters, using virt-deploy to create the VM
    and netlaunch to deploy the OS.
    """
    if not installer_iso_path:
        argus_runcmd(["make", "build/installer-dev.iso"])
        installer_iso_path = ARGUS_REPO_DIR_PATH / "build/installer-dev.iso"

    host_config_path = prepare_hostconfig(test_dir_path, ssh_key_path)

    remote_addr_path = test_dir_path / REMOTE_ADDR_FILENAME
    deploy_os(host_config_path, remote_addr_path, installer_iso_path)

    # Add the VMs SSH key to known_hosts. TODO use a predictable key instead
    inject_ssh_key(remote_addr_path, known_hosts_path)

    ssh_node = create_ssh_node(remote_addr_path, ssh_key_path, known_hosts_path)
    disable_phonehome(ssh_node)

    return ssh_node


def create_ssh_node(remote_addr_path: Path, ssh_key_path: Path, known_hosts_path: Path):
    """Creates an SSH node that can be used to interact with the VM."""
    with open(remote_addr_path, "r") as file:
        remote_addr = file.read().strip()
    return SshNode(
        ".",
        "log",
        remote_addr,
        username=TEST_USER,
        key_path=ssh_key_path,
        known_hosts_path=known_hosts_path,
    )


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


def argus_runcmd(cmd, check=True, **kwargs):
    """Runs a command in the argus repository directory."""
    logging.debug(f"Running command: {cmd}")
    subprocess.run(cmd, check=check, cwd=ARGUS_REPO_DIR_PATH, **kwargs)


@pytest.fixture(scope="package")
def vm(request):
    """Define the VirtDeploy based LibVirt VM as a resource the tests can use."""

    keep_environment = request.config.getoption("--keep-environment")
    reuse_environment = request.config.getoption("--reuse-environment")
    test_dir_path = request.config.getoption("--test-dir")
    ssh_key_path = request.config.getoption("--ssh-key")
    build_output = request.config.getoption("--build-output")
    force_upload = request.config.getoption("--force-upload")
    redeploy = request.config.getoption("--redeploy")

    installer_iso_path = None
    if os.environ.get("INSTALLER_ISO_PATH"):
        installer_iso_path = os.path.abspath(os.environ["INSTALLER_ISO_PATH"])

    if (
        not ARGUS_REPO_DIR_PATH.is_dir()
        or not (ARGUS_REPO_DIR_PATH / "virt-deploy").is_file()
    ):
        raise Exception(
            "{} is not a argus-toolkit repo directory".format(ARGUS_REPO_DIR_PATH)
        )

    if not ssh_key_path:
        raise Exception("Must specify --ssh-key pointing to an existing SSH key")
    ssh_key_path = Path(ssh_key_path)
    if not ssh_key_path.is_file():
        raise Exception("SSH key file does not exist")

    if build_output:
        build_output = Path(build_output)
        if not build_output.is_file():
            raise Exception("Build output file does not exist")

    if reuse_environment:
        if not test_dir_path or not os.path.isdir(test_dir_path):
            raise Exception(
                "Must specify --test-dir pointing to an existing test directory when using --reuse-environment"
            )
        test_dir_path = Path(test_dir_path)
    else:
        test_dir_path = setup_testdir(test_dir_path)

    known_hosts_path = test_dir_path / KNOWN_HOSTS_FILENAME

    if reuse_environment:
        # Create SSH Node for the existing VM.
        ssh_node = create_ssh_node(
            test_dir_path / REMOTE_ADDR_FILENAME, ssh_key_path, known_hosts_path
        )
    else:
        create_vm(["-d", "16,16"])

    if not reuse_environment or redeploy:
        # Deploy OS to VM.
        ssh_node = deploy_vm(
            test_dir_path,
            ssh_key_path,
            known_hosts_path,
            installer_iso_path,
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

    # Fixture cleanup.
    argus_runcmd([ARGUS_REPO_DIR_PATH / "virt-deploy", "create", "--clean"])
