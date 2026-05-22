---
sidebar_position: 6
---

# E2E Tests

E2E tests validate complete Trident install-and-update workflows using
`netlaunch` to boot a VM from an installer ISO, followed by pytest validation
of the resulting host state. They are defined by Host Configurations and test
selections in `tests/e2e_tests/trident_configurations/`.

E2E tests are orchestrated by two systems:

- **storm-trident** (`bin/storm-trident`): A Go-based test orchestrator built on
  the [Storm](https://github.com/microsoft/storm) framework. It manages VM
  lifecycle, runs `netlaunch` for clean installs, and coordinates multi-step
  update scenarios.
- **pytest e2e suite** (`tests/e2e_tests/`): A Python test suite that validates
  the host state after servicing operations (partitions, filesystems, boot
  order, etc.) by connecting to the VM over SSH.

For VM-image-based servicing and rollback tests that don't use netlaunch, see
[Servicing Tests](Servicing-Tests.md) and [Rollback Tests](Rollback-Tests.md).

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

See [Dependencies](Dependencies.md) for full build dependency details including
protobuf compiler requirements.

## Building Dependencies

### 1. Build Trident

```bash
make build
```

### 2. Build Go Tools

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

# Build the usr-verity test image (for UKI/systemd-boot configurations)
sudo ./tests/images/testimages.py build trident-usrverity-testimage --output-dir ./artifacts/test-image
```

The images use the Image Customizer container from
`mcr.microsoft.com/azurelinux/imagecustomizer:latest`.

### COSI Image Types

E2E tests validate three COSI image types, each testing a different bootloader
and integrity configuration:

| Image | Output File | Bootloader | Integrity | Config |
|-------|------------|-----------|-----------|--------|
| `trident-testimage` | `regular.cosi` | grub2 | None | `tests/images/trident-testimage/base/baseimg.yaml` |
| `trident-verity-testimage` | `verity.cosi` | grub2 | Root dm-verity | `tests/images/trident-verity-testimage/base/baseimg.yaml` |
| `trident-usrverity-testimage` | `usrverity.cosi` | systemd-boot | `/usr` dm-verity (UKI) | `tests/images/trident-verity-testimage/usr/host.yaml` |

**`regular.cosi`** — Standard grub2-based image with no integrity protection.
Uses `grub2-efi-binary-noprefix`, includes `trident-service` and
`tridentd.socket`. This is the baseline image for most E2E configurations.

**`verity.cosi`** — Root filesystem is protected by dm-verity, making `/`
read-only. Uses grub2 with a separate `/var` partition and an `/etc` overlay
service for runtime state. Includes `veritysetup` and `dracut-overlayfs`.

**`usrverity.cosi`** — The `/usr` filesystem is protected by dm-verity, with
a Unified Kernel Image (UKI) and systemd-boot as the bootloader. This is a
preview feature (`previewFeatures: uki`). Requires `ukify` on the build host.

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

After a successful install, use `storm-trident` to perform an A/B update.
The `ab-update` helper requires SSH credentials — you must supply the key path
explicitly (there is no default):

```bash
bin/storm-trident helper ab-update \
    --ssh-host <VM_IP> \
    --ssh-key <PATH_TO_SSH_PRIVATE_KEY> \
    --ssh-user testing-user \
    -c /var/lib/trident/config.yaml \
    -v 2 \
    -s -f
```

For netlaunch-based installs, the SSH key is typically the one you added to the
Host Configuration (e.g., `~/.ssh/id_rsa`).

Flags:
- `--ssh-host`: IP address of the VM (printed by netlaunch after install)
- `--ssh-key`: Path to the SSH private key used for VM access
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

## Running E2E Validation Tests (pytest)

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
- `-K`: Path to SSH key (defaults to `tests/e2e_tests/helpers/key`). This file
  is not checked into the repo — you must create or symlink it before running,
  or pass `-K` explicitly with your key path.
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

## Automated E2E Scenarios

For automated multi-step scenarios (install → update → validate → rollback),
use storm-trident's scenario mode. All E2E scenarios use the Storm framework and
share the same underlying code.

### Listing Scenarios

```bash
# List all E2E scenarios
bin/storm-trident list scenarios -t e2e

# Filter by hardware and runtime
bin/storm-trident list scenarios -t e2e -t vm        # VM scenarios only
bin/storm-trident list scenarios -t e2e -t container  # Container runtime only
```

### Scenario Naming

All E2E scenarios follow the naming convention:

```
<config>_<hardware>-<runtime>
```

Where:
- `<config>` is the name of the host config (e.g., `base`, `simple`,
  `usrverity`).
- `<hardware>` is either `vm` (virtual machine) or `bm` (bare metal).
- `<runtime>` is either `host` (runs directly on the host) or `container`
  (runs inside a container).

For example: `base_vm-host`, `combined_vm-container`.

### Running a Scenario

```bash
bin/storm-trident run <scenario-name> -- <parameters>
```

To see available parameters for any scenario:

```bash
bin/storm-trident run <scenario-name> -- --help
```

Common parameters:

| Flag | Description |
|------|-------------|
| `--iso` | Path to the installer ISO |
| `-i, --test-image-dir` | Directory containing test COSI images (default: `./artifacts/test-image`) |
| `--logstream-file` | File to write logstream to (default: `logstream-full.log`) |
| `--tracestream-file` | File to write tracestream to |
| `--signing-cert` | Path to certificate for VM EFI variables |
| `--dump-ssh-key` | Dump SSH private key to a file for debugging |
| `--vm-wait-for-login-timeout` | Timeout for VM login prompt |
| `--test-ring` | Test ring to filter test cases |

### Test Rings

E2E scenarios are organized into test rings that control how frequently they run:

- **pr-e2e**: Run on every pull request (innermost ring)
- **post_merge**: Run after merge to main
- **daily**: Run daily
- **weekly**: Run weekly
- **full-validation**: Run for release validation (outermost ring)

Rings are cumulative — all scenarios in inner rings also run when an outer ring
is executed.

### How E2E Discovery Works

E2E scenario discovery automatically finds all configured Host Configurations
and determines when each should run. The key components:

- **Configuration definitions**: All Host Configurations live in
  `tests/e2e_tests/trident_configurations/`, and the mapping of which
  configurations run in which test rings is defined in
  `tests/e2e_tests/target-configurations.yaml`.
- **Discovery function**: `DiscoverTridentE2EScenarios` in
  `tools/storm/e2e/discover.go` produces instances of `TridentE2EScenario`
  (from `tools/storm/e2e/scenario/trident.go`) for each valid combination of
  Host Configuration, hardware type, and runtime.
- **Go embed**: Discovery uses Go's `go:generate` and `go:embed` directives to
  copy configurations into the binary. The `invert.py` script in
  `tools/storm/e2e/` produces `configurations/configurations.yaml` with the
  structure:

  ```yaml
  <config_name>:
     <hardware_type>:
       <runtime>: <lowest_pipeline_ring>
  ```

  For example:

  ```yaml
  base:
    vm:
      host: pr-e2e
  ```

- **Special config parameters**: Configurations can be customized with YAML keys
  defined in `TridentE2EHostConfigParams` (in `tools/storm/e2e/scenario/trident.go`),
  such as `maxExpectedFailures` for configs that may have intermittent failures.

### Matrix Generation in Pipelines

The `e2e-matrix` script (in `tools/storm/e2e/matrix_script.go`) generates ADO
pipeline job matrices from discovered scenarios. It takes a test ring as input
and outputs one matrix per hardware/runtime combination as ADO variables:

```bash
bin/storm-trident script e2e-matrix pr-e2e
```

Variable names follow the pattern `TEST_MATRIX_E2E_<HARDWARE>_<RUNTIME>` (e.g.,
`TEST_MATRIX_E2E_VM_HOST`).

### Pipeline Execution

E2E tests in CI are orchestrated by two pipeline templates:

- `.pipelines/templates/stages/testing_e2e/storm_e2e.yml` — entry point that
  invokes matrix generation and dispatches test execution jobs.
- `.pipelines/templates/stages/testing_e2e/test_execution_template.yml` — runs
  individual E2E scenarios from the generated matrix.

### E2E Test Code

All E2E test logic lives under `tools/storm/e2e/scenario/`. The main entry point
is `trident.go`, which contains the `TridentE2EScenario` struct implementing the
Storm Scenario interface.
