---
sidebar_position: 6
---

# E2E Tests

E2E tests validate complete Trident install-and-update workflows using
`netlaunch` to boot a QEMU VM from an installer ISO, followed by pytest
validation of the resulting host state. They are defined by Host Configurations
and test selections in `tests/e2e_tests/trident_configurations/`.

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

## COSI Image Types

E2E tests validate three COSI image types, each testing a different bootloader
and integrity configuration:

| Image | Output File | Bootloader | Integrity | Configurations |
|-------|------------|-----------|-----------|----------------|
| `trident-testimage` | `regular.cosi` | grub2 | None | base, encrypted-partition, encrypted-raid, encrypted-swap, extensions, health-checks-install, misc, raid-big, raid-mirrored, raid-resync-small, raid-small, simple, split |
| `trident-verity-testimage` | `verity.cosi` | grub2 | Root dm-verity | root-verity |
| `trident-usrverity-testimage` | `usrverity.cosi` | systemd-boot | `/usr` dm-verity (UKI) | combined, memory-constraint-combined, rerun, usr-verity, usr-verity-raid |

**`regular.cosi`** — Standard grub2-based image with no integrity protection.
Uses `grub2-efi-binary-noprefix`, includes `trident-service` and
`tridentd.socket`. This is the baseline image for most E2E configurations.

**`verity.cosi`** — Root filesystem is protected by dm-verity, making `/`
read-only. Uses grub2 with a separate `/var` partition and an `/etc` overlay
service for runtime state. Includes `veritysetup` and `dracut-overlayfs`.

**`usrverity.cosi`** — The `/usr` filesystem is protected by dm-verity, with
a Unified Kernel Image (UKI) and systemd-boot as the bootloader. This is a
preview feature (`previewFeatures: uki`). Requires `ukify` on the build host.

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

See [Dependencies](../Building/Dependencies.md) for full build dependency details including
protobuf compiler requirements.

Unless otherwise noted, commands are run from the repository root. Pytest
commands are run from `tests/e2e_tests`.

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
make bin/netlisten       # Serves COSI images for A/B updates
make bin/storm-trident   # E2E test orchestrator
make bin/virtdeploy      # VM lifecycle management
make bin/isopatch        # Injects files into ISOs
make bin/rcp-agent       # Remote control plane agent
```

### 3. Build osmodifier

```bash
make artifacts/osmodifier
```

### 4. Build Trident RPMs

The test images include Trident packages built from your local tree. This step
builds the RPMs into `bin/RPMS/`, which `testimages.py` passes to Image
Customizer via `--rpm-source`:

```bash
make bin/trident-rpms.tar.gz
```

This requires Docker and uses the Trident packaging Dockerfile to produce
RPMs from your compiled binary and osmodifier.

### 5. Generate SSH Keys

```bash
make artifacts/id_rsa
```

### 6. Download Base Image

```bash
# Downloads baremetal.vhdx from MCR
./tests/images/testimages.py download-image baremetal
```

### 7. Build COSI Images

Build the test COSI images that Trident will install and update. A/B updates
require two images with unique filesystem UUIDs — Trident rejects updates where
the new image matches the installed one. Use `--clones 2` to produce two images,
then rename them into `artifacts/test-image/`:

```bash
mkdir -p artifacts/test-image

# Build two clones (produces trident-testimage_0.cosi and trident-testimage_1.cosi)
sudo ./tests/images/testimages.py build trident-testimage \
    --output-dir ./artifacts/test-image --clones 2

# Rename clones to the filenames referenced by Host Configurations
mv artifacts/test-image/trident-testimage_0.cosi artifacts/test-image/regular.cosi
mv artifacts/test-image/trident-testimage_1.cosi artifacts/test-image/regular_v2.cosi
```

Repeat for other image types as needed:

```bash
# Verity image (for root-verity configuration)
sudo ./tests/images/testimages.py build trident-verity-testimage \
    --output-dir ./artifacts/test-image --clones 2
mv artifacts/test-image/trident-verity-testimage_0.cosi artifacts/test-image/verity.cosi
mv artifacts/test-image/trident-verity-testimage_1.cosi artifacts/test-image/verity_v2.cosi

# UKI/usr-verity image (for usr-verity, combined configurations)
sudo ./tests/images/testimages.py build trident-usrverity-testimage \
    --output-dir ./artifacts/test-image --clones 2
mv artifacts/test-image/trident-usrverity-testimage_0.cosi artifacts/test-image/usrverity.cosi
mv artifacts/test-image/trident-usrverity-testimage_1.cosi artifacts/test-image/usrverity_v2.cosi
```

The images use the Image Customizer container from
`mcr.microsoft.com/azurelinux/imagecustomizer:latest`.

### 8. Extract Image Customizer

The MOS ISO build uses the Image Customizer binary directly (not the
container). Extract it from the container image:

```bash
mkdir -p artifacts
id=$(docker create mcr.microsoft.com/azurelinux/imagecustomizer:latest)
docker cp "$id:/usr/bin/imagecustomizer" artifacts/imagecustomizer
docker rm "$id"
chmod +x artifacts/imagecustomizer
```

### 9. Build the Installer ISO

The management OS ISO is used by `netlaunch` to boot the VM and run Trident:

```bash
make bin/trident-mos.iso
```

## Running a Clean Install

### 1. Create the QEMU VM

Use `virtdeploy` to create a VM with empty disks:

```bash
sudo bin/virtdeploy create-one --disks 32,8
```

This creates a QEMU/libvirt VM and writes `tools/vm-netlaunch.yaml` with the
VM UUID. The `--disks` flag specifies disk sizes in GB (here, 32 GB for the OS
disk and 8 GB for a secondary disk).

### 2. Create a Host Configuration

Use the starter configuration as a template:

```bash
make starter-configuration
```

This copies `tests/e2e_tests/trident_configurations/simple/trident-config.yaml`
to `input/trident.yaml`. Edit it to add your SSH public key under
`os.users[0].sshPublicKeys`.

Alternatively, copy a configuration directly from any test configuration
directory:

```bash
mkdir -p input
cp tests/e2e_tests/trident_configurations/<config>/trident-config.yaml input/trident.yaml
```

The Host Configuration references image URLs like
`http://NETLAUNCH_HOST_ADDRESS/files/regular.cosi`. Netlaunch automatically
replaces `NETLAUNCH_HOST_ADDRESS` with its own `<host-IP>:<port>` at runtime,
so the VM can reach the files served by netlaunch.

### 3. Run netlaunch

```bash
NETLAUNCH_PORT=4000 make run-netlaunch
```

:::important
Set `NETLAUNCH_PORT` to a fixed port (e.g., `4000`). A/B updates use
`netlisten` to serve update images, and it must listen on the **same port** as
netlaunch — the host configuration on the VM retains the original
`<host>:<port>` URL from the install.
:::

Netlaunch will:

- Validate the Host Configuration
- Symlink `tools/vm-netlaunch.yaml` to `input/netlaunch.yaml`
- Boot the QEMU VM from the installer ISO
- Serve `artifacts/test-image/` over HTTP (including COSI images)
- Patch the Host Configuration with the server address
- Stream Trident's install logs to the terminal

Netlaunch blocks until the install completes and the VM reboots into the
installed OS. Do not stop it until you see the VM reboot. The VM IP address
is printed in the output (typically `192.168.242.2` for the default
`192.168.242.0/24` network).

To use a custom ISO or config:

```bash
NETLAUNCH_PORT=4000 NETLAUNCH_ISO=path/to/custom.iso TRIDENT_CONFIG=path/to/config.yaml make run-netlaunch
```

### 4. Watch the VM console (optional)

In a separate terminal:

```bash
make watch-virtdeploy
```

## Running E2E Validation (pytest)

After an install or update, validate the host state with the pytest suite.
Run from the `tests/e2e_tests` directory:

### After Clean Install

```bash
cd tests/e2e_tests
python3 -u -m pytest -m daily --capture=no \
    -H <VM_IP> \
    -R host \
    -C trident_configurations/<config> \
    -K ../../artifacts/id_rsa \
    -v
```

### After A/B Update

The `-A` flag specifies which volume is active after the update. Volumes
alternate with each update: a clean install boots `volume-a`, the first A/B
update switches to `volume-b`, the next back to `volume-a`, and so on.

```bash
cd tests/e2e_tests
python3 -u -m pytest -m daily --capture=no \
    -H <VM_IP> \
    -R host \
    -C trident_configurations/<config> \
    -K ../../artifacts/id_rsa \
    -A <active-volume> \
    -v
```

Where `<active-volume>` is `volume-b` after the first update, `volume-a` after
the second, and so on.

### Flags

| Flag | Long Form | Description | Default |
|------|-----------|-------------|---------|
| `-m daily` | | Pytest marker filter — selects the `daily` test ring | (required) |
| `--capture=no` | | Disables output capture (avoids conflicts with fabric SSH) | `fd` |
| `-H` | `--host` | IP address or hostname of the target VM | (required) |
| `-R` | `--runtime-env` | Runtime environment: `host` or `container` | `host` |
| `-C` | `--configuration` | Path to configuration directory | (required) |
| `-K` | `--keypath` | Path to SSH private key | `tests/e2e_tests/helpers/key` |
| `-A` | `--ab-active-volume` | Active A/B volume: `volume-a` or `volume-b` | `volume-a` |
| `-S` | `--expected-host-status-state` | Expected Trident servicing state | `provisioned` |
| `-v` | `--verbose` | Verbose test output | off |

:::note
The default key path (`tests/e2e_tests/helpers/key`) is not checked into the
repo. Always pass `-K` explicitly with your key path, or create a symlink at
the default location.
:::

## Running an A/B Update

After a successful clean install, perform an A/B update using `netlisten` to
serve the update image and `storm-trident` to orchestrate the update.

### 1. Start netlisten

In a separate terminal, start `netlisten` to serve the update images. It
**must** use the same port that netlaunch used during install:

```bash
sudo bin/netlisten \
    -s artifacts/test-image \
    -p 4000 \
    -m trident-ab-update-metrics.jsonl \
    -b logstream-ab-update.log
```

### 2. Run storm-trident A/B update

```bash
sudo bin/storm-trident helper ab-update -- \
    artifacts/id_rsa \
    <VM_IP> \
    testing-user \
    host \
    -c /var/lib/trident/config.yaml \
    -v 2 \
    -s -f
```

The `ab-update` helper takes four positional arguments followed by flags:

| Argument | Description |
|----------|-------------|
| `<private-key-path>` | Path to the SSH private key |
| `<host>` | IP address of the VM |
| `<user>` | SSH user on the VM |
| `<trident-runtime-type>` | `host` or `container` |

| Flag | Description | Default |
|------|-------------|---------|
| `-c` | Path to the Host Configuration file **on the VM** | (required) |
| `-v` | Version number for the update image (appended as `_v<N>` suffix) | (required) |
| `-s` | Stage the A/B update | `false` |
| `-f` | Finalize the A/B update (triggers reboot) | `false` |
| `-p` | SSH port | `22` |
| `-t` | SSH connection timeout in seconds | `600` |

**Success looks like:**

```
=== SUMMARY of storm-trident::helper::ab-update ===
  get-config...........: PASS
  update-hc............: PASS
  trigger-update.......: PASS
  check-trident-service: PASS
  check-diagnostics....: PASS
=== RESULT ===
OK: passed: 5; total: 5
```

After the update succeeds, run the [post-update pytest validation](#after-ab-update)
with `-A volume-b`.

### Other storm-trident helpers

```bash
bin/storm-trident helper manual-rollback -- <key> <host> <user> <runtime> ...
bin/storm-trident helper check-selinux -- <key> <host> <user> <runtime> ...
bin/storm-trident helper display-logs -- <key> <host> <user> <runtime> ...
bin/storm-trident helper rebuild-raid -- <key> <host> <user> <runtime> ...
```

Run `bin/storm-trident helper <name> -- --help` to see available flags.

## Cleanup

Destroy the QEMU VM and network:

```bash
sudo bin/virtdeploy clean
```

## Test Configurations

Pre-defined test configurations live in `tests/e2e_tests/trident_configurations/`.
Each subdirectory contains:

- `trident-config.yaml`: The Host Configuration to install
- `test-selection.yaml`: Which test markers to enable (e.g., `compatible: [base]`)

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

All E2E scenarios follow the naming convention `<config>_<hardware>-<runtime>`:

- `<config>`: Name of the host config (e.g., `base`, `simple`, `usrverity`)
- `<hardware>`: `vm` (virtual machine) or `bm` (bare metal)
- `<runtime>`: `host` (runs directly on the host) or `container` (runs inside a container)

For example: `base_vm-host`, `combined_vm-container`.

### Running a Scenario

```bash
bin/storm-trident run <scenario-name> -- <parameters>
```

To see available parameters:

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

- **Special config parameters**: Configurations can be customized with YAML keys
  defined in `TridentE2EHostConfigParams` (in `tools/storm/e2e/scenario/trident.go`),
  such as `maxExpectedFailures` for configs that may have intermittent failures.

### Matrix Generation in Pipelines

The `e2e-matrix` script generates ADO pipeline job matrices from discovered
scenarios:

```bash
bin/storm-trident script e2e-matrix pr-e2e
```

Variable names follow the pattern `TEST_MATRIX_E2E_<HARDWARE>_<RUNTIME>` (e.g.,
`TEST_MATRIX_E2E_VM_HOST`).

### E2E Test Code

All E2E test logic lives under `tools/storm/e2e/scenario/`. The main entry point
is `trident.go`, which contains the `TridentE2EScenario` struct implementing the
Storm Scenario interface.
