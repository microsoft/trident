---
sidebar_position: 6
---

# E2E Tests

End-to-end (E2E) tests validate complete Trident workflows — clean install,
A/B update, rollback, encryption, verity, extensions, and more — on real
virtual machines using production-like images.

E2E tests are orchestrated by two systems:

- **storm-trident** (`bin/storm-trident`): A Go-based test orchestrator built on
  the [Storm](https://github.com/microsoft/storm) framework. It manages VM
  lifecycle, runs `netlaunch` for clean installs, and coordinates multi-step
  servicing scenarios including A/B updates and rollbacks.
- **pytest e2e suite** (`tests/e2e_tests/`): A Python test suite that validates
  the host state after servicing operations (partitions, filesystems, boot
  order, etc.) by connecting to the VM over SSH.

## Overview

A typical E2E test run follows this flow:

1. **Build** COSI images (install and update images) using Image Customizer.
2. **Build** an installer ISO (the management OS that boots and runs Trident).
3. **Create** a QEMU/libvirt VM with empty disks.
4. **Install** the OS using `netlaunch`, which boots the VM from the ISO, serves
   the COSI image and Host Configuration over HTTP, and streams Trident logs.
5. **Validate** the installation using the pytest e2e suite.
6. **Update** the OS using `storm-trident` A/B update helper, which uploads a
   new Host Configuration and COSI image, triggers `trident update`, and waits
   for reboot.
7. **Validate** the update using the pytest e2e suite.

## Prerequisites

- **Linux host** with root access
- **libvirt and QEMU** installed and configured (user must be in the `libvirt`
  group)
- **Docker** (for building images with Image Customizer)
- **[oras](https://oras.land/)** CLI (for downloading base images from MCR)
- **Go 1.24+** (for building Go tools)
- **Rust** (latest stable, for building Trident)
- **Python 3.8+** with packages:

  ```bash
  pip3 install fabric pyyaml pytest
  ```

## Building Dependencies

### 1. Build Trident

```bash
make build
```

### 2. Build Go Tools

Build the tools required for E2E testing:

```bash
# Build all Go tools at once
make go-tools

# Or build individually:
make bin/netlaunch       # Boots VM from ISO, serves config over HTTP
make bin/storm-trident   # E2E test orchestrator
make bin/virtdeploy      # VM lifecycle management
make bin/isopatch        # Injects files into ISOs
make bin/rcp-agent       # Remote control plane agent
```

### 3. Build osmodifier

```bash
make artifacts/osmodifier
```

### 4. Download Base Image

```bash
# Downloads baremetal.vhdx from MCR
./tests/images/testimages.py download-image baremetal
```

### 5. Build COSI Images

Build the test COSI images that Trident will install/update:

```bash
# Build the regular test image
sudo ./tests/images/testimages.py build trident-testimage --output-dir ./artifacts/test-image

# Build the verity test image (for verity configurations)
sudo ./tests/images/testimages.py build trident-verity-testimage --output-dir ./artifacts/test-image
```

The images use the Image Customizer container from
`mcr.microsoft.com/azurelinux/imagecustomizer:latest`.

### 6. Build the Installer ISO

The management OS ISO is used by `netlaunch` to boot the VM and run Trident:

```bash
make bin/trident-mos.iso
```

This builds an Azure Linux ISO with Trident and its dependencies baked in.

## Running a Clean Install with netlaunch

`netlaunch` orchestrates a bare-metal-style install: it boots a QEMU VM from the
installer ISO, serves the Host Configuration and COSI image over HTTP, and
streams Trident's logs back to the terminal.

### 1. Create a Host Configuration

Use the starter configuration as a template:

```bash
make starter-configuration
```

This copies `tests/e2e_tests/trident_configurations/simple/trident-config.yaml`
to `input/trident.yaml`. Edit it to add your SSH public key under
`os.users[0].sshPublicKeys`.

The Host Configuration references `http://NETLAUNCH_HOST_ADDRESS/files/regular.cosi`
as the image URL. `netlaunch` automatically replaces `NETLAUNCH_HOST_ADDRESS`
with its own IP and port at runtime.

### 2. Run netlaunch

```bash
make run-netlaunch
```

This will:
- Validate the Host Configuration
- Boot a QEMU VM from the installer ISO
- Serve `artifacts/test-image/` over HTTP (including COSI images)
- Patch the Host Configuration with the server address
- Stream Trident's install logs to the terminal
- Wait for the VM to reboot into the installed OS

To use a custom ISO or config:

```bash
NETLAUNCH_ISO=path/to/custom.iso TRIDENT_CONFIG=path/to/config.yaml make run-netlaunch
```

### 3. Watch the VM console (optional)

In a separate terminal:

```bash
make watch-virtdeploy
```

## Running an A/B Update with storm-trident

After a successful install, use `storm-trident` to perform an A/B update:

```bash
bin/storm-trident helper ab-update \
    --ssh-host <VM_IP> \
    --ssh-key tests/e2e_tests/helpers/key \
    --ssh-user testing-user \
    -c /var/lib/trident/config.yaml \
    -v 2 \
    -s -f
```

Flags:
- `--ssh-host`: IP address of the VM (printed by netlaunch after install)
- `-c`: Path to the Host Configuration file on the VM
- `-v`: Version number for the update image
- `-s`: Stage the update
- `-f`: Finalize the update (triggers reboot)

Other useful storm-trident helpers:

```bash
# Manual rollback
bin/storm-trident helper manual-rollback --ssh-host <VM_IP> ...

# Check SELinux status
bin/storm-trident helper check-selinux --ssh-host <VM_IP> ...

# Display Trident logs
bin/storm-trident helper display-logs --ssh-host <VM_IP> ...

# Rebuild RAID
bin/storm-trident helper rebuild-raid --ssh-host <VM_IP> ...
```

Run `bin/storm-trident helper --help` to see all available helpers.

## Running E2E Validation Tests

After an install or update, validate the host state with the pytest suite:

```bash
cd tests/e2e_tests
python3 -m pytest \
    -H <VM_IP> \
    -C trident_configurations/simple \
    -v
```

Flags:
- `-H`: IP address or hostname of the target machine
- `-C`: Path to the configuration directory (contains `trident-config.yaml` and
  `test-selection.yaml`)
- `-K`: Path to SSH key (defaults to `tests/e2e_tests/helpers/key`)
- `-R`: Runtime environment — `host` or `container` (default: `host`)
- `-A`: Active A/B volume — `volume-a` or `volume-b` (default: `volume-a`)

## Test Configurations

Pre-defined test configurations live in `tests/e2e_tests/trident_configurations/`.
Each subdirectory contains:

- `trident-config.yaml`: The Host Configuration to install
- `test-selection.yaml`: Which test markers to enable

Available configurations include: `simple`, `base`, `combined`,
`encrypted-partition`, `encrypted-raid`, `extensions`, `raid-mirrored`,
`root-verity`, `usr-verity`, `split`, and more.

The full matrix of which configurations run in which pipeline tier is defined in
`tests/e2e_tests/target-configurations.yaml`.

## Running a Full E2E Scenario with storm-trident

For automated multi-step scenarios (install → update → validate → rollback),
use storm-trident's scenario mode. The scenarios are defined in
`tools/storm/e2e/` and are what the CI pipelines run:

```bash
bin/storm-trident scenario <scenario-name> [flags]
```

Run `bin/storm-trident scenario --help` to see available scenarios.
