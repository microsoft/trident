#!/usr/bin/env python3
# Copyright (c) Microsoft Corporation.

import argparse
import sys
import time
from os import getcwd
from os.path import basename, dirname, abspath, join
from pathlib import Path
from paramiko import client, ed25519key, ChannelException, SSHException
from paramiko import rsakey as rsa
from paramiko.ssh_exception import AuthenticationException, NoValidConnectionsError
import logging
from fabric import Connection, Config
from fabric.transfer import Transfer

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s %(levelname)-8s %(message)s",
    datefmt="%Y-%m-%d %H:%M:%S",
)
logging.getLogger("paramiko").setLevel(logging.WARNING)

logging.info("Current sys path: %s", sys.path)

current_working_dir = getcwd()
logging.info("Current working directory: %s", getcwd())

# Get the current file directory
current_dir = dirname(abspath(__file__))
logging.info("Current File Directory: %s", current_dir)

library_functions_path = join(current_working_dir, "..", "bare-metal", "scripts")
sys.path.append(library_functions_path)
logging.info("Library Functions Path: %s", library_functions_path)

from az_cli import *
from baremetal_provisioner import BMC, os_provision, BareMetalHost, get_system_logs
from baremetal_config import baremetal_config
from network_interface import NetworkInterface
from deploy import (
    get_baremetal_config,
    get_ssh_public_key,
    generate_cloud_init,
    get_ssh_public_key,
    upload_iso_to_azure,
    upload_iso_to_httpd_server,
    validate_url,
    update_download_job_host_image,
)


def wait_for_ssh_connection(ssh_connection, timeout=900):
    wait_time_secs = 30
    stop_time = time.monotonic() + timeout
    while True:
        try:
            ssh_connection.open()
            logging.info("ssh connection successful.")
        except (
            ChannelException,
            SSHException,
            AuthenticationException,
            TimeoutError,
            ValueError,
            NoValidConnectionsError,
        ) as ex:
            if time.monotonic() >= stop_time:
                raise Exception("timeout - ssh connection failed!") from ex
            else:
                logging.info(f"Error trying to open ssh connection: {str(ex)}")
                logging.info(f"Waiting for ssh to {ssh_connection.host}...")
                time.sleep(wait_time_secs)
                continue

        break


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--trident-yaml",
        required=True,
        help="Path to the trident.yaml to use for provisioning",
    )
    parser.add_argument(
        "--installer-iso",
        required=True,
        help="Path to the installer ISO to use for provisioning",
    )
    parser.add_argument(
        "--baremetal-config", required=True, help="Config of the baremetal machine"
    )
    parser.add_argument(
        "--proxy", required=False, help="Proxy to use if required", default=None
    )

    # In case of using trident image, the variable names start with trident
    parser.add_argument(
        "--trident-httpd-ip",
        dest="httpd_ip",
        help="IP address of the LiveCD HTTP server. If not specified, this script will use Azure Storage Account for uploading built ISO.",
    )
    parser.add_argument(
        "--trident-httpd-username",
        dest="httpd_username",
        default="ubuntu",
        help="Username on the LiveCD HTTPD server",
    )
    parser.add_argument(
        "--trident-httpd-ssh-key",
        dest="httpd_ssh_key",
        help="Private SSH key for the LiveCD HTTPD server",
    )
    args = parser.parse_args()

    if args.httpd_ip and not args.httpd_ssh_key:
        raise Exception(
            "Not enough input args. --trident-httpd-ssh-key needs to be provided when using --trident-httpd-ip"
        )

    config = get_baremetal_config(args.baremetal_config)

    # user data
    ssh_username = config.nodes.bootstrap.cloud_init.ssh_username
    logging.info(f"ssh_username: {ssh_username}")
    ssh_key = config.nodes.bootstrap.cloud_init.ssh_key

    # get public key
    ssh_public_key = get_ssh_public_key(ssh_key)

    # network data
    network = config.nodes.bootstrap.network
    oam_network = NetworkInterface(
        "oam",
        network.oam.mac_address,
        network.oam.ip,
        network.oam.gateway,
        False,
        network.oam.dns,
    )

    ip = oam_network.ip

    logging.info("Trident deployment, skipping generating livecd iso and cloud-init")
    iso_path = args.installer_iso

    # TODO: Task#6389 Uncomment when cloud-init is supported in runtime OS.
    # TODO: Currently it is not supported.
    # generate_cloud_init(ssh_username, ssh_public_key,
    #             oam_network, args.k8s_version,
    #             args.proxy, trident_yaml=args.trident_yaml)

    if args.httpd_ip:
        # upload the generated live_cd_iso to provided HTTPD server
        # For debugging: use iso_url = "http://10.248.0.4/isodir/afo-host-live-cd-20230310-221709.iso"
        # The above ISO has a plaintext password for each debugging
        iso_url = upload_iso_to_httpd_server(
            args.httpd_ip, args.httpd_username, args.httpd_ssh_key, iso_path
        )
    else:
        # upload the generated live_cd_iso to Azure storage and generate sas
        # For debugging: use iso_url = "https://releases.ubuntu.com/22.04.1/ubuntu-22.04.1-desktop-amd64.iso"
        iso_url = upload_iso_to_azure(iso_path)

    logging.info(f"Generated ISO URL: {iso_url}")
    validate_url(iso_url)

    # on some versions of iDRACs, remote share mount requires the share to end with ".iso"
    # otherwise an error occurs parsing the url. As a workaround, append a no-op .iso
    if not iso_url.endswith(".iso"):
        iso_url += "&.iso"

    bmc_info = BMC(
        config.nodes.bootstrap.bmc.ip,
        config.nodes.bootstrap.bmc.username,
        config.nodes.bootstrap.bmc.password,
    )
    machine = BareMetalHost(bmc_info)
    os_provision(bareMetalMachine=machine, iso_url=iso_url)

    try:
        # attempt ssh till host is up
        config = Config(
            overrides={
                "connect_kwargs": {"key_filename": ssh_key, "look_for_keys": False}
            }
        )
        ssh_connection = Connection(host=ip, user=ssh_username, config=config)
        wait_for_ssh_connection(ssh_connection, timeout=1020)
    finally:
        get_system_logs(bareMetalMachine=machine)

    logging.info("Bootstrap node is up!")


if __name__ == "__main__":
    main()
