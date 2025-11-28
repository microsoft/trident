
# Network Configuration

Trident uses [netplan](https://netplan.io/) to configure the network on the
host. Netplan is a utility for easily configuring networking on a Linux system
via YAML files. It works by generating backend-specific configuration files for
either NetworkManager or systemd-networkd, depending on the system's
configuration.

Trident generates netplan configuration files based on the
[`netplan`](../Reference/Host-Configuration/API-Reference/Os.md#netplan-optional)
section of the Host Configuration file. This section allows you to specify
network configuration by directly specifying a
[Netplan YAML Configuration](https://netplan.readthedocs.io/en/stable/netplan-yaml/).
