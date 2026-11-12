#!/usr/bin/env python3

import argparse
import hashlib
import ipaddress
import json
import subprocess
import sys
import xml.etree.ElementTree as ET

PRIVATE_POOLS = (
    ipaddress.ip_network("10.0.0.0/8"),
    ipaddress.ip_network("172.16.0.0/12"),
    ipaddress.ip_network("192.168.0.0/16"),
)
SUBNET_PREFIX = 24


class NetworkSelectionError(RuntimeError):
    pass


def run_command(arguments):
    try:
        return subprocess.run(
            arguments,
            check=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        ).stdout
    except (OSError, subprocess.CalledProcessError) as error:
        raise NetworkSelectionError(
            f"Failed to run {' '.join(arguments)}: {error}"
        ) from error


def host_route_networks():
    output = run_command(["ip", "-j", "-4", "route", "show", "table", "all"])
    routes = json.loads(output)
    networks = []
    for route in routes:
        destination = route.get("dst")
        if not destination or destination == "default":
            continue
        try:
            networks.append(ipaddress.ip_network(destination, strict=False))
        except ValueError as error:
            raise NetworkSelectionError(
                f"Invalid IPv4 route returned by ip: {destination}"
            ) from error
    return networks


def libvirt_xml_networks(xml):
    networks = []
    root = ET.fromstring(xml)
    for element in root.findall("ip"):
        address = element.get("address")
        if not address or ":" in address:
            continue
        prefix = element.get("prefix")
        netmask = element.get("netmask")
        if prefix:
            network = f"{address}/{prefix}"
        elif netmask:
            network = f"{address}/{netmask}"
        else:
            continue
        networks.append(ipaddress.ip_network(network, strict=False))
    return networks


def libvirt_networks():
    output = run_command(
        ["virsh", "-c", "qemu:///system", "net-list", "--all", "--name"]
    )
    networks = []
    for name in output.splitlines():
        name = name.strip()
        if not name:
            continue
        xml = run_command(["virsh", "-c", "qemu:///system", "net-dumpxml", name])
        try:
            networks.extend(libvirt_xml_networks(xml))
        except ET.ParseError as error:
            raise NetworkSelectionError(
                f"Invalid XML returned for libvirt network {name}"
            ) from error
    return networks


def candidate_networks(namespace):
    pool_offsets = []
    max_subnet_count = 0
    for pool in PRIVATE_POOLS:
        subnet_count = 1 << (SUBNET_PREFIX - pool.prefixlen)
        digest = hashlib.sha256(f"{namespace}:{pool.with_prefixlen}".encode()).digest()
        pool_offsets.append(int.from_bytes(digest[:8], "big") % subnet_count)
        max_subnet_count = max(max_subnet_count, subnet_count)

    for attempt in range(max_subnet_count):
        for pool, offset in zip(PRIVATE_POOLS, pool_offsets):
            subnet_count = 1 << (SUBNET_PREFIX - pool.prefixlen)
            if attempt >= subnet_count:
                continue
            subnet_index = (offset + attempt) % subnet_count
            network_address = int(pool.network_address) + (
                subnet_index << (32 - SUBNET_PREFIX)
            )
            yield ipaddress.ip_network((network_address, SUBNET_PREFIX))


def select_network(namespace, unavailable_networks, requested_network=None):
    if requested_network:
        try:
            candidate = ipaddress.ip_network(requested_network, strict=False)
        except ValueError as error:
            raise NetworkSelectionError(
                f"Invalid requested network: {requested_network}"
            ) from error
        if candidate.version != 4:
            raise NetworkSelectionError("The requested network must be IPv4")
        if any(candidate.overlaps(network) for network in unavailable_networks):
            raise NetworkSelectionError(
                f"Requested network {candidate} overlaps an existing host route "
                "or libvirt network"
            )
        return candidate

    for candidate in candidate_networks(namespace):
        if not any(candidate.overlaps(network) for network in unavailable_networks):
            return candidate
    raise NetworkSelectionError("No unused private /24 network is available")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("namespace")
    parser.add_argument("--network")
    args = parser.parse_args()

    unavailable_networks = host_route_networks() + libvirt_networks()
    network = select_network(args.namespace, unavailable_networks, args.network)
    print(network)


if __name__ == "__main__":
    try:
        main()
    except NetworkSelectionError as error:
        print(error, file=sys.stderr)
        sys.exit(1)
