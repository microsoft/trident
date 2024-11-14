#!/usr/bin/env python3
# Copyright (c) Microsoft Corporation.

import argparse
from typing import Optional
import yaml

import logging


def update_trident_host_config(
    host_configuration: str,
    oam_ip: str,
    interface_name: str,
    oam_gateway: Optional[str] = None,
    oam_mac: Optional[str] = None,
):
    logging.info("Updating host config section of trident.yaml")
    logging.info("oam_ip: %s", oam_ip)
    logging.info("oam_gateway: %s", oam_gateway)
    os = host_configuration.setdefault("os", {})
    network = os.setdefault("network", {})
    ethernets = network.setdefault("ethernets", {})
    eno_interface = ethernets.setdefault(interface_name, {})

    # Temporary fix for #8837.
    if oam_mac:
        eno_interface["match"] = {"macaddress": oam_mac}

    eno_interface.setdefault("addresses", []).append(oam_ip + "/23")
    eno_interface["dhcp4"] = True
    if oam_gateway:
        eno_interface.setdefault("routes", []).append(
            {"to": "0.0.0.0/0", "via": oam_gateway}
        )

    logging.info("Updating os disks device in trident.yaml")
    disks = host_configuration.get("storage", {}).get("disks", [])
    for disk in disks:
        if disk["id"] == "os":
            disk["device"] = "/dev/sda"
        elif disk["id"] == "disk2":
            disk["device"] = "/dev/sdb"

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
    args = parser.parse_args()

    with open(args.trident_yaml) as f:
        trident_yaml_content = yaml.safe_load(f)

    update_trident_host_config(
        trident_yaml_content,
        args.oam_ip,
        args.interface_name,
        args.oam_gateway,
        args.oam_mac,
    )
    with open(args.trident_yaml, "w") as f:
        yaml.dump(trident_yaml_content, f, default_flow_style=False)


if __name__ == "__main__":
    main()
