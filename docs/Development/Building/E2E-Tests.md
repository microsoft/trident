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

### 4. Generate SSH Keys

Several scenarios require an SSH key pair in `artifacts/`:

```bash
make artifacts/id_rsa
```

### 5. Download Base Image

```bash
# Downloads baremetal.vhdx from MCR
./tests/images/testimages.py download-image baremetal
```

### 6. Build COSI Images

Build the test COSI images that Trident will install/update:

```bash
# Build the regular test image
sudo ./tests/images/testimages.py build trident-testimage --output-dir ./artifacts/test-image

# Build the verity test image (for verity configurations)
sudo ./tests/images/testimages.py build trident-verity-testimage --output-dir ./artifacts/test-image
```

The images use the Image Customizer container from
`mcr.microsoft.com/azurelinux/imagecustomizer:latest`.

### 7. Build the Installer ISO

The management OS ISO is used by `netlaunch` to boot the VM and run Trident:

```bash
make bin/trident-mos.iso
```

This builds an Azure Linux ISO with Trident and its dependencies baked in.

### 8. Build Trident RPMs (for VM-based scenarios)

The servicing and rollback scenarios require Trident RPMs baked into VM images:

```bash
make bin/trident-rpms.tar.gz
```

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
Host Configuration (e.g., `~/.ssh/id_rsa`). For servicing and rollback
scenarios, it is `artifacts/id_rsa` (generated by `make artifacts/id_rsa`).

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

## E2E Scenarios with storm-trident

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

## Servicing Scenario

The servicing scenario tests multi-update workflows on a pre-built VM image. It
deploys a VM from a QCOW2 image, then runs an update loop with rollback testing.

### Test Cases

The servicing scenario runs these test cases in order:

1. **publish-sig-image** — Publishes the image to Azure SIG (Azure platform only,
   skipped for QEMU)
2. **deploy-vm** — Creates and boots a QEMU VM from a QCOW2 artifact
3. **check-deployment** — Verifies the VM deployed successfully
4. **update-loop** — Runs repeated A/B updates, staging and finalizing each one
5. **rollback** — Tests rollback after update (when `--rollback` is enabled)
6. **collect-logs** — Fetches logs from the VM
7. **cleanup-vm** — Destroys the VM

### Running Locally

```bash
bin/storm-trident run servicing -- \
    --artifacts-dir ./artifacts \
    --output-path /tmp/servicing-output \
    --platform qemu \
    --ssh-private-key-path ./artifacts/id_rsa \
    --verbose
```

Key flags:

| Flag | Description | Default |
|------|-------------|---------|
| `--artifacts-dir` | Directory containing VM images and COSI files | `/tmp` |
| `--output-path` | Output directory for logs | `./output` |
| `--platform` | `qemu` or `azure` | `qemu` |
| `--ssh-private-key-path` | Path to SSH private key | `~/.ssh/id_rsa` |
| `--rollback` | Enable rollback testing | `false` |
| `--retry-count` | Number of update retry attempts | `3` |
| `--verbose` | Enable verbose logging | `false` |
| `--force-cleanup` | Force VM cleanup on exit | `false` |

## Rollback Scenario

The rollback scenario tests manual rollback and runtime updates end-to-end. It
builds a modified QCOW2 with extensions, runs A/B and runtime updates, then
rolls back through each one verifying state at every step.

### What It Tests

1. Start a VM with sysext extension v1
2. Verify extension is active, active volume is A
3. Run an A/B update with sysext extension v2 and new netplan
4. Verify extension is v2, netplan is correct, active volume is B
5. Run a runtime update with sysext extension v3 and new netplan
6. Verify extension is v3, active volume is still B
7. Run a runtime update removing sysext and netplan
8. Verify extension and netplan are gone
9. Roll back runtime update #2 → verify v3 is restored
10. Roll back runtime update #1 → verify v2 is restored
11. Roll back A/B update → verify v1, active volume is A

### Building Dependencies

```bash
# Choose the test image
TEST_IMAGE_NAME="trident-vm-usr-verity-testimage"
# Alternative: TEST_IMAGE_NAME="trident-vm-grub-verity-testimage"
# (grub variant skips extension testing since IC cannot add extensions to the
# original QCOW2)

# Build storm-trident
make bin/storm-trident

# Build the test sysext extension images
pushd ./artifacts
../bin/storm-trident script build-extension-images --build-sysexts --num-clones 3
popd

# Build Trident RPMs
sudo rm -f bin/trident-rpms.tar.gz
sudo rm -rf bin/RPMS
make bin/trident-rpms.tar.gz

# Clean any previous test images
sudo rm -f artifacts/trident-vm-*-testimage.qcow2 artifacts/trident-vm-*-testimage.cosi

# Generate SSH keys (if not already present)
make artifacts/id_rsa

# Build the required test images (COSI + QCOW2)
make artifacts/$TEST_IMAGE_NAME.cosi
make artifacts/$TEST_IMAGE_NAME.qcow2
```

### Running Locally

```bash
sudo ./bin/storm-trident run rollback -w --verbose \
    --artifacts-dir ./artifacts/ \
    --output-path /tmp/output \
    --platform qemu \
    --ssh-private-key-path ./artifacts/id_rsa \
    --ssh-public-key-path ./artifacts/id_rsa.pub
```

Optional skip flags:

| Flag | Description |
|------|-------------|
| `--skip-runtime-updates` | Skip runtime update testing |
| `--skip-manual-rollbacks` | Skip manual rollback testing |
| `--skip-extension-testing` | Skip extension testing |
| `--skip-netplan-runtime-testing` | Skip netplan runtime update testing |

:::warning
When using `trident-vm-grub-verity-testimage`, add `--skip-extension-testing`
since Image Customizer cannot add extensions to the original QCOW2 for that
image type.
:::
