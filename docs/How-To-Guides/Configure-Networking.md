
# Configure Networking

Configure networking in the target OS.

## Goals

- Configure networking, interfaces, IP addresses, gateways, routes, DNS, etc. in
  the target OS.

## Prerequisites

1. Trident installed on the servicing OS.
2. Netplan must be installed on the target OS.

   With Image Customizer, you can add netplan to your image by adding the
   following to your Image Customizer configuration:

   ```yaml
   os:
     packages:
       install:
         - netplan
   ```

:::warning
cloud-init also supports configuring networking. If cloud-init is installed on
the target OS, it may override netplan configuration. Trident will attempt to
disable cloud-init networking, but you may want to disable it completely if
you are experiencing network configuration issues.
:::

## Instructions

### Step 1: Add a Networking Section to the HC

To configure networking in the target OS, you need to add the `netplan:` section
to the `os:` section of your Host Configuration file.

```yaml
os:
  # (other OS configuration sections ...)
  netplan:
    version: 2
    # Configuration goes here!
```

:::info
The `version` field is required and must always be set to `2`.
:::

### Step 2: Configure Network Interfaces in Host Configuration

Enter any valid netplan configuration under the `netplan:` section of your Host
Configuration file to configure networking and interfaces as desired. Netplan
YAML documentation is available here:
[Netplan YAML configuration](https://netplan.readthedocs.io/en/stable/netplan-yaml/).

Here is an example configuration for setting up eth0 to use DHCP:

```yaml
os:
  netplan:
    version: 2
    ethernets:
      eth0:  # Netplan will match this interface by name
        dhcp4: true
```

To setup all interfaces with names matching `e*` to use DHCP:

```yaml
os:
  netplan:
    version: 2
    ethernets:
      dhcpInterfaces: # This name is arbitrary
        match:
          name: e* # Netplan will match all interfaces with names starting with 'e'
        dhcp4: true
```

To setup a static IP address on eth0:

```yaml
os:
  netplan:
    version: 2
    ethernets:
      eth0:  # Netplan will match this interface by name
        addresses:
          - <IP>/<MASK>
        gateway4: <GATEWAY_IP>
        nameservers:
          addresses:
            - <DNS1_IP>
            - # ...
```

### Step 3: Apply the Host Configuration

Once the `netplan` section is added to your Host Configuration file,
apply the configuration to the target OS using Trident.

Depending on the servicing type being performed, this will be a call to either
`trident install` or `trident update`.

## Troubleshooting

Netplan supports a wide variety of configurations. When running into issues with
more complicated setups, you may want to test your configuration using netplan
directly by extracting the netplan section from your HC file, putting it into
its own YAML file in `/etc/netplan/`, and running:

```bash
sudo netplan generate
```
