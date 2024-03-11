import subprocess
import time
import pytest
import os
import tempfile
import logging
import yaml

from pathlib import Path

from .conftest import (
    argus_runcmd,
    ARGUS_REPO_DIR_PATH,
    TRIDENT_REPO_DIR_PATH,
    VM_SSH_NODE_CACHE_KEY,
)
from .ssh_node import SshNode


def create_vm(create_params):
    """Creates a VM with the given parameters, using virt-deploy."""
    argus_runcmd([ARGUS_REPO_DIR_PATH / "virt-deploy", "create"] + create_params)
    argus_runcmd([ARGUS_REPO_DIR_PATH / "virt-deploy", "run"])


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


def disable_phonehome(ssh_node: SshNode):
    """Disables phonehome in the VM to allow faster rerunning of Trident."""
    ssh_node.execute("sudo sed -i 's/phonehome: .*//' /etc/trident/config.yaml")


def prepare_hostconfig(test_dir_path: Path, ssh_pub_key: str):
    """Sets up the host configuration file for the VM."""

    # Add user's public key to trident-setup.yaml
    with open(
        TRIDENT_REPO_DIR_PATH / "functional_tests/trident-setup.yaml", "r"
    ) as file:
        trident_setup = yaml.safe_load(file)
    trident_setup["hostConfiguration"]["os"]["users"][0]["sshPublicKeys"].clear()
    trident_setup["hostConfiguration"]["os"]["users"][0]["sshPublicKeys"].append(
        ssh_pub_key
    )

    prepped_host_config_path = test_dir_path / "trident-setup.yaml"
    with open(prepped_host_config_path, "w") as file:
        yaml.dump(trident_setup, file)

    return prepped_host_config_path


def deploy_vm(
    test_dir_path: Path,
    ssh_pub_key: str,
    known_hosts_path: Path,
    installer_iso_path: Path,
    remote_addr_path: Path,
) -> str:
    """# Provision a VM with the given parameters, using virt-deploy to create the VM
    and netlaunch to deploy the OS. Returns the ip address of the VM.
    """
    if not installer_iso_path:
        argus_runcmd(["make", "build/netlaunch"])
        argus_runcmd(["make", "build/installer-dev.iso"])
        installer_iso_path = ARGUS_REPO_DIR_PATH / "build/installer-dev.iso"

    host_config_path = prepare_hostconfig(test_dir_path, ssh_pub_key)

    deploy_os(host_config_path, remote_addr_path, installer_iso_path)

    # Temporary solution to initialize the known_hosts file until we can inject
    # a predictable key.
    with open(remote_addr_path, "r") as file:
        remote_addr = file.read().strip()

    for i in range(10):
        try:
            with open(known_hosts_path, "w") as file:
                subprocess.run(["ssh-keyscan", remote_addr], stdout=file, check=True)
            break
        except:
            time.sleep(1)

    return remote_addr


def test_create_vm(request):
    """Test function to create a VM with virt-deploy"""
    request.config.cache.set(VM_SSH_NODE_CACHE_KEY, None)
    if not request.config.getoption("--reuse-environment"):
        # Create one VM with default flags, cpus, memory, but with two 16GiB disks.
        create_vm([":::16,16"])


@pytest.mark.depends("test_create_vm")
def test_deploy_vm(
    request,
    test_dir_path,
    reuse_environment,
    redeploy,
    remote_addr_path,
    known_hosts_path,
    ssh_key_public,
):
    if reuse_environment and not redeploy:
        # Get the IP address from the remote_addr file of the existing VM.
        with open(remote_addr_path, "r") as file:
            address = file.read().strip()
    else:
        installer_iso_path = None
        if os.environ.get("INSTALLER_ISO_PATH"):
            installer_iso_path = os.path.abspath(os.environ["INSTALLER_ISO_PATH"])

        if (
            not ARGUS_REPO_DIR_PATH.is_dir()
            or not (ARGUS_REPO_DIR_PATH / "virt-deploy").is_file()
        ):
            pytest.fail(f"{ARGUS_REPO_DIR_PATH} is not a argus-toolkit repo directory")

        # Deploy OS to VM.
        address = deploy_vm(
            test_dir_path,
            ssh_key_public,
            known_hosts_path,
            installer_iso_path,
            remote_addr_path,
        )

    request.config.cache.set(VM_SSH_NODE_CACHE_KEY, address)


@pytest.mark.depends("test_deploy_vm")
def test_deployment(vm):
    vm.execute("true")
