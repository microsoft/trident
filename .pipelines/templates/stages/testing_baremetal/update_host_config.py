#!/usr/bin/env python3
# Copyright (c) Microsoft Corporation.

import argparse
from typing import Optional
import yaml

import logging


def wait_online_script(interface_name: str) -> str:
    """
    Generates a script to add a wait for the given network interface to be
    online before starting the Trident service.
    """
    return "\n".join(
        [
            "set -eux",
            f"systemctl enable systemd-networkd-wait-online@{interface_name}.service",
            "mkdir -p /etc/systemd/system/trident.service.d",
            "cat << EOF > /etc/systemd/system/trident.service.d/override.conf",
            "[Unit]",
            f"Requires=systemd-networkd-wait-online@{interface_name}.service",
            "EOF",
        ]
    )


def update_trident_host_config(
    *,
    host_configuration: str,
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

    logging.info("Updating os disks device in trident.yaml")
    disks = host_configuration.get("storage", {}).get("disks", [])
    for disk in disks:
        if disk["id"] == "os":
            disk["device"] = "/dev/sda"
        elif disk["id"] == "disk2":
            disk["device"] = "/dev/sdb"

    host_configuration.setdefault("scripts", {}).setdefault("postConfigure", []).append(
        {
            "content": wait_online_script(interface_name),
            "name": "wait-for-network",
            "runOn": ["all"],
        }
    )

    logging.info(
        "Final trident_yaml content post all the updates: %s", host_configuration
    )


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
    with open(args.trident_yaml, "w") as f:
        yaml.dump(trident_yaml_content, f, default_flow_style=False)


if __name__ == "__main__":
    main()
