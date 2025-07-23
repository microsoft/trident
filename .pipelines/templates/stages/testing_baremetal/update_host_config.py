#!/usr/bin/env python3
# Copyright (c) Microsoft Corporation.

import argparse
from typing import Optional
import yaml

import logging


def update_trident_host_config(
    *,
    host_configuration: dict,
    interface_name: str,
    interface_ip: str,
    interface_mac: Optional[str] = None,
    network_gateway: Optional[str] = None,
    use_dhcp: bool = False,
):
    logging.info("Updating host config section of trident.yaml")
    os = host_configuration.setdefault("os", {})

    main_interface = {
        "addresses": [f"{interface_ip}/23"],
        "dhcp4": use_dhcp,
        "set-name": interface_name,
    }

    # Temporary fix for #8837.
    if interface_mac:
        main_interface["match"] = {"macaddress": interface_mac}

    if network_gateway:
        main_interface.setdefault("routes", []).append(
            {"to": "0.0.0.0/0", "via": network_gateway}
        )

    # Override network to only preserve the eno interface.
    os["network"] = {
        "version": 2,
        "ethernets": {
            interface_name: main_interface,
        },
    }

    # Name of the wait online service for this interface
    wait_online_service = f"systemd-networkd-wait-online@{interface_name}.service"

    # Enable systemd-networkd-wait-online service for the interface.
    enable_services = os.setdefault("services", {}).setdefault("enable", [])
    if wait_online_service not in enable_services:
        enable_services.append(wait_online_service)

    # Add an override for the trident service to wait for the network
    # interface to be online before starting.
    os.setdefault("additionalFiles", []).append(
        {
            "destination": "/etc/systemd/system/trident.service.d/override.conf",
            "content": "[Unit]\n"
            f"Requires={wait_online_service}\n"
            f"After={wait_online_service}\n",
        }
    )

    logging.info("Updating os disks device in trident.yaml")
    disks = host_configuration.get("storage").get("disks")
    for disk in disks:
        if disk["id"] == "os":
            disk["device"] = "/dev/sda"
        elif disk["id"] == "disk2":
            disk["device"] = "/dev/sdb"

    # If this is root verity, we need to set an internal param to be able to
    # configure the network.
    if is_root_verity(host_configuration):
        logging.info(
            "Detected root verity configuration, setting 'writableEtcOverlayHooks' internal param."
        )
        host_configuration.setdefault("internalParams", {})[
            "writableEtcOverlayHooks"
        ] = True

    logging.info(
        "Final trident_yaml content post all the updates: %s", host_configuration
    )

    # TODO: Remove this logic once pcrlock encryption is enabled for the BM
    # scenario. For now, if this is a UKI image, add an internal param to
    # disable pcrlock encryption. Related ADO task:
    # https://dev.azure.com/mariner-org/polar/_workitems/edit/14269/.
    if host_configuration.get("internalParams", {}).get("uki", False):
        logging.info(
            "Detected UKI image, setting 'overridePcrlockEncryption' internal param."
        )
        host_configuration.setdefault("internalParams", {})[
            "overridePcrlockEncryption"
        ] = True


def is_root_verity(host_configuration: dict) -> bool:
    """
    Check if the host configuration is using root verity.
    """

    verity_config = host_configuration.get("storage", {}).get("verity", [])
    if len(verity_config) == 0:
        return False

    if len(verity_config) > 1:
        raise ValueError("Multiple verity configurations found, expected only one.")

    verity = verity_config[0]
    verity_id = verity.get("id")

    filesystems = host_configuration.get("storage", {}).get("filesystems", [])
    verity_filesystem = None
    for fs in filesystems:
        if fs.get("deviceId") == verity_id:
            verity_filesystem = fs
            break

    if verity_filesystem is None:
        return False

    mount_point = verity_filesystem.get("mountPoint")
    if mount_point is None:
        return False
    if isinstance(mount_point, str):
        return mount_point == "/"
    if isinstance(mount_point, dict):
        return mount_point.get("path") == "/"

    return False


def main():
    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s %(levelname)-8s %(message)s",
        datefmt="%Y-%m-%d %H:%M:%S",
    )
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--trident-yaml",
        required=True,
        help="Path to the trident.yaml to use for provisioning",
    )
    parser.add_argument(
        "--oam-ip", required=True, help="IP address of the OAM interface."
    )
    parser.add_argument(
        "--interface-name",
        default="eno8303",
        help="Interface Name that needs the IP assigned. Default: eno8303",
    )
    parser.add_argument(
        "--oam-gateway", default=None, help="IP address of the OAM gateway."
    )
    parser.add_argument(
        "--oam-mac", default=None, help="MAC address of the OAM interface."
    )
    parser.add_argument("--use-dhcp", default=False, help="Configure DHCP.")
    parser.add_argument(
        "-o",
        "--output",
        default=None,
        help="Output file path. Defaults to editing the input file.",
    )
    args = parser.parse_args()

    with open(args.trident_yaml) as f:
        trident_yaml_content = yaml.safe_load(f)

    update_trident_host_config(
        host_configuration=trident_yaml_content,
        interface_name=args.interface_name,
        interface_ip=args.oam_ip,
        interface_mac=args.oam_mac,
        network_gateway=args.oam_gateway,
        use_dhcp=args.use_dhcp,
    )

    output_path = args.output or args.trident_yaml
    with open(output_path, "w") as f:
        yaml.dump(trident_yaml_content, f, default_flow_style=False)


if __name__ == "__main__":
    main()
