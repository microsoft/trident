import json
import os
import logging
import subprocess
import tempfile
import time

from typing import Dict
from pathlib import Path
from subprocess import CalledProcessError, TimeoutExpired

from .conftest import (
    trident_runcmd,
    TRIDENT_REPO_DIR_PATH,
    VM_SSH_NODE_CACHE_KEY,
    FT_BASE_IMAGE,
    TEST_USER,
)
from .ssh_node import SshNode

log = logging.getLogger(__name__)

CLOUD_INIT_USER_TEMPLATE = """
#cloud-config
users:
  - name: {username}
    ssh_authorized_keys:
      - {ssh_pub_key}
    sudo: ['ALL=(ALL) NOPASSWD:ALL']
"""


def create_vm(create_params) -> Dict[str, str]:
    log.info("Creating VM with parameters: %s", create_params)
    """Creates a VM with the given parameters, using virt-deploy."""
    out = trident_runcmd(
        [TRIDENT_REPO_DIR_PATH / "bin" / "virtdeploy", "create-one", "-J"]
        + create_params,
        capture_output=True,
        text=True,
    )

    metadata = json.loads(out.stdout)

    return metadata["vms"][0]


def wait_online(ip: str, known_hosts_path: Path, timeout: int = 60) -> None:
    """Waits for the VM to be online by checking SSH connectivity."""
    start_time = time.time()
    while time.time() - start_time < timeout:
        try:
            with open(known_hosts_path, "w") as f:
                subprocess.run(
                    [
                        "ssh-keyscan",
                        ip,
                    ],
                    stdout=f,
                    check=True,
                    timeout=5,
                )
            return
        except (CalledProcessError, TimeoutExpired) as e:
            time.sleep(5)

    raise TimeoutError(f"VM with IP {ip} did not come online within {timeout} seconds.")


def test_create_vm(request, known_hosts_path, ssh_key_public):
    """Test function to create a VM with virt-deploy"""

    if request.config.getoption("--reuse-environment"):
        log.info("Skipping VM creation as --reuse-environment is set.")
        return

    request.config.cache.set(VM_SSH_NODE_CACHE_KEY, None)

    with tempfile.TemporaryDirectory() as temp_dir:
        work_dir = Path(temp_dir)
        # Create a cloud-init metadata file for the VM.
        cloud_init_meta = work_dir / "cloud-init-meta.yaml"
        with open(cloud_init_meta, "w") as file:
            file.write("#cloud-config\n")

        # Create a cloud-init user-data file for the VM.
        cloud_init_user_data = work_dir / "cloud-init-user-data.yaml"
        with open(cloud_init_user_data, "w") as file:
            file.write(
                CLOUD_INIT_USER_TEMPLATE.format(
                    username=TEST_USER,
                    ssh_pub_key=ssh_key_public,
                )
            )

        # Create one VM with default flags, cpus, memory, but with two 16GiB
        # disks. Pass the base Image as the OS disk. And Pass cloud init-params
        # to set up a user and ssh access.
        vm_data = create_vm(
            [
                "-d",
                "16,16",
                "--os-disk",
                FT_BASE_IMAGE,
                "--ci-user",
                cloud_init_user_data,
                "--ci-meta",
                cloud_init_meta,
            ]
        )

    vm_name = vm_data["name"]
    vm_ip = vm_data["ip"]

    subprocess.run(
        ["virsh", "start", vm_name],
        check=True,
    )

    wait_online(vm_ip, known_hosts_path, timeout=60)

    request.config.cache.set(VM_SSH_NODE_CACHE_KEY, vm_ip)


# def test_wait_online(known_hosts_path):
#     wait_online("192.168.242.2", known_hosts_path, timeout=60)


def test_deployment(vm: SshNode):
    vm.execute("true")
