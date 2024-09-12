#!/usr/bin/env python3
# Copyright (c) Microsoft Corporation.

import argparse
from os.path import basename
from pathlib import Path
import urllib
import yaml
import subprocess

import logging


def update_trident_host_config(
    trident_yaml_content,
    iso_httpd_ip,
    oam_ip,
    netlisten_port,
    ssh_pub_key,
    interface_name,
    host_interface,
    oam_gateway="",
):
    logging.info("Updating host config section of trident.yaml")
    logging.info("iso_httpd_ip: %s", iso_httpd_ip)
    logging.info("oam_ip: %s", oam_ip)
    logging.info("oam_gateway: %s", oam_gateway)
    host_configuration = trident_yaml_content.get("hostConfiguration")
    os = host_configuration.setdefault("os", {})
    network = os.setdefault("network", {})
    ethernets = network.setdefault("ethernets", {})
    eno_interface = ethernets.setdefault(interface_name, {})

    # Temporary fix for #8837.
    eno_interface["match"] = {"macaddress": "c8:4b:d6:7a:73:c6"}

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

    def update_image_url(image):
        new = image["url"].replace(
            "NETLAUNCH_HOST_ADDRESS/files", f"{iso_httpd_ip}/isodir/hermes-image"
        )
        logging.info(f"Image found. Updating source url: {image['url']} -> {new}")
        image["url"] = new

    logging.info("Updating source url in filesystems")
    for fs in host_configuration.get("storage").get("filesystems", []):
        logging.info(f"Checking filesystem: {fs}")
        source = fs.get("source")
        if source and source.get("type") in ["image", "esp-image"]:
            update_image_url(source)

    logging.info("Updating source url in verity filesystems")
    for fs in host_configuration.get("storage").get("verityFilesystems", []):
        logging.info(f"Updating verity filesystem: {fs}")
        update_image_url(fs.get("dataImage"))
        update_image_url(fs.get("hashImage"))

    logging.info("Updating mariner_user in trident.yaml")
    users = os.setdefault("users", [])
    users.append(
        {"name": "mariner_user", "sshPublicKeys": [ssh_pub_key], "sshMode": "key-only"}
    )

    logging.info("Updating phonehome and logstream in trident.yaml")
    # Get inet address of the interface
    output = subprocess.run(
        ["ip", "addr", "show", host_interface], text=True, capture_output=True
    )
    output = output.stdout.split("\n")[2].strip()
    logging.info(f"Output of ip addr show {host_interface}: {output}")
    netlisten_address = output.split(" ")[1].split("/")[0]
    logging.info(f"Netlisten address: {netlisten_address}")
    trident_yaml_content["phonehome"] = (
        f"http://{netlisten_address}:{netlisten_port}/phonehome"
    )
    trident_yaml_content["logstream"] = (
        f"http://{netlisten_address}:{netlisten_port}/logstream"
    )

    logging.info(
        "Final trident_yaml content post all the updates: %s", trident_yaml_content
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
        "--iso-httpd-ip", required=True, help="IP address of the HTTP server."
    )
    parser.add_argument(
        "--oam-ip", required=True, help="IP address of the OAM interface."
    )
    parser.add_argument(
        "--netlisten-port",
        required=True,
        help="Port to use for netlisten.",
    )
    parser.add_argument(
        "--ssh-pub-key", required=True, help="SSH public key to use for provisioning."
    )
    parser.add_argument(
        "--interface-name",
        default="eno8303",
        help="Interface Name that needs the IP assigned. Default: eno8303",
    )
    parser.add_argument(
        "--host-interface",
        default="eth0",
        help="Host interface to use for netlisten. Default: eth0",
    )
    parser.add_argument(
        "--oam-gateway", default="", help="IP address of the OAM gateway."
    )
    args = parser.parse_args()
    with open(args.ssh_pub_key) as f:
        ssh_pub_key_content = f.read()

    with open(args.trident_yaml) as f:
        trident_yaml_content = yaml.safe_load(f)

    update_trident_host_config(
        trident_yaml_content,
        args.iso_httpd_ip,
        args.oam_ip,
        args.netlisten_port,
        ssh_pub_key_content.strip().strip("\n"),
        args.interface_name,
        args.host_interface,
        args.oam_gateway,
    )
    with open(args.trident_yaml, "w") as f:
        yaml.dump(trident_yaml_content, f, default_flow_style=False)


if __name__ == "__main__":
    main()
