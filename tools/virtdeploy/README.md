# VirtDeploy

## Overview

Virtdeploy is a tool to create VMs to mock real physical servers.

## Summary of How it Works

Note: "Deploy Agent" is whatever you are using to deploy an OS through the BMC, eg. `netlaunch`.

```
:::mermaid
sequenceDiagram
actor you
participant dep as Deploy Agent
participant vdc as VirtDeploy Create
participant vdr as VirtDeploy Run
participant fs as Filesystem
participant vdr as VirtDeploy Run
participant ipt as iptables
participant docker as Docker
participant sushy as BMC (Sushy Tools)
participant lv as Libvirt

rect rgb(60, 2, 82)
note right of you: Starting VirtDeploy Create
you->>vdc: Invoke
activate vdc
vdc->>lv: Cleanup old resources
vdc->>lv: Create virtual routed network
vdc->>lv: Create virtual storage pool
vdc->>lv: Create storage volumes
vdc->>lv: Create domains
lv-->>vdc: Domains metadata
vdc-->>fs: Write metadata to file
deactivate vdc
end

rect rgb(17, 54, 6)
note right of you: Starting VirtDeploy Run
you->>vdr: Invoke
activate vdr
activate vdr
fs-->>vdr: Domain Metadata
vdr->>ipt: Create NAT rule for virtual network
vdr->>docker: Create a sushy-container per domain
docker->>sushy: Start
docker-->>vdr: Container IP
vdr-->>fs: Write BMC metadata
deactivate vdr
end

rect rgb(130, 61, 0)
note right of you: Deploying with your agent
you->>dep: Invoke
activate dep
fs-->>dep: Get BMC metadata
dep->>sushy: Upload virtual media
activate sushy
sushy->>lv: Attach CD to domain
sushy->>lv: Set CD as default boot
deactivate sushy
dep->>sushy: Start host
activate sushy
sushy->>lv: Start host
deactivate sushy
deactivate dep
end
:::
```

## Dependencies

On Ubuntu:

If you do not have docker installed yet, first, get docker:

```bash
curl -fsSL https://get.docker.com | sudo bash
```

If you are on Ubuntu **20.04**, you will need to install this PPA for `swtpm`:

```bash
sudo add-apt-repository ppa:stefanberger/swtpm-focal -y
sudo apt-get update
```

```bash
sudo NEEDRESTART_MODE=a apt-get install -y \
    virt-manager \
    qemu-efi \
    python3-libvirt \
    ovmf \
    openssl \
    python3-netifaces \
    python3-docker \
    python3-bcrypt \
    python3-jinja2 \
    swtpm \
    swtpm-tools \
    curl
```

## Setup

Add yourself to the required groups:

```bash
sudo usermod -aG docker $USER
sudo usermod -a -G libvirt $USER
```

*NOTE: Linux groups are only reloaded at log-in. After this you need to re-log in.*

Set libvirt URI for your user:

```bash
mkdir -p ~/.config/libvirt
cat << EOF > ~/.config/libvirt/libvirt.conf
uri_default = "qemu:///system"
EOF
```

## Running

### Resource creation

```bash
./tools/virt-deploy create
```

*Note: Use `--help` to see more options!*
