---
sidebar_position: 6
---

# E2E Tests

E2E tests validate complete Trident install-and-update workflows using
`netlaunch` to boot a QEMU VM from an installer ISO, followed by pytest
validation of the resulting host state. They are defined by Host Configurations
and test selections in `tests/e2e_tests/trident_configurations/`.

Trident supports two **runtime environments**:

- **Host** — The Trident binary runs directly on the host OS. The installer ISO
  (`trident-mos.iso`) and test COSI images include Trident RPMs.
- **Container** — The Trident binary runs inside a Docker container on the host
  OS. The container image (`trident-container.tar.gz`) is served alongside the
  COSI images and loaded at runtime. Container scenarios use a different
  installer ISO (`trident-container-installer.iso`) and different COSI test
  images that include Docker but omit Trident RPMs.

Both runtimes use the same test configurations, pytest suite, and storm-trident
orchestrator — only the build artifacts and a few flags differ. This page covers
both; differences are called out where they apply.

E2E tests are orchestrated by three systems:

- **storm-trident** (`bin/storm-trident`): A Go-based test orchestrator built on
  the [Storm](https://github.com/microsoft/storm) framework. It manages VM
  lifecycle, runs `netlaunch` for clean installs, and coordinates multi-step
  update scenarios.
- **pytest e2e suite** (`tests/e2e_tests/`): A Python test suite that validates
  the host state after servicing operations (partitions, filesystems, boot
  order, etc.) by connecting to the VM over SSH.
- **Test configurations** (`tests/e2e_tests/trident_configurations/`): A
  declarative system that pairs Host Configurations with test selections. Each
  configuration directory defines *what* to install (disk layout, filesystems,
  features) and *which tests* to run against it, enabling the same pytest suite
  to validate many different Trident deployment scenarios.

For VM-image-based servicing and rollback tests that don't use netlaunch, see
[Servicing Tests](Servicing-Tests.md) and [Rollback Tests](Rollback-Tests.md).

## COSI Image Types

E2E tests validate multiple COSI image types, each testing a different
bootloader and integrity configuration. Each image type has a **host** variant
(includes Trident RPMs) and a **container** variant (includes Docker, omits
Trident RPMs).

### Host Runtime Images

| Image | Output File | Bootloader | Integrity | Configurations |
|-------|------------|-----------|-----------|----------------|
| `trident-testimage` | `regular.cosi` | grub2 | None | base, encrypted-partition, encrypted-raid, encrypted-swap, extensions, health-checks-install, misc, raid-big, raid-mirrored, raid-resync-small, raid-small, simple, split |
| `trident-verity-testimage` | `verity.cosi` | grub2 | Root dm-verity | root-verity |
| `trident-usrverity-testimage` | `usrverity.cosi` | systemd-boot | `/usr` dm-verity (UKI) | combined, memory-constraint-combined, rerun, usr-verity, usr-verity-raid |

### Container Runtime Images

| Image | Output File | Bootloader | Integrity | Configurations |
|-------|------------|-----------|-----------|----------------|
| `trident-container-testimage` | `container.cosi` | grub2 | None | base, encrypted-partition, encrypted-raid, encrypted-swap, extensions, health-checks-install, misc, raid-mirrored, raid-resync-small, raid-small, simple |
| `trident-container-verity-testimage` | `container-verity.cosi` | grub2 | Root dm-verity | root-verity |
| `trident-container-usrverity-testimage` | `container-usrverity.cosi` | systemd-boot | `/usr` dm-verity (UKI) | combined, rerun, usr-verity, usr-verity-raid |

Container test images do **not** include Trident RPMs — Trident runs from a
Docker container (`trident-container.tar.gz`) loaded at runtime. The images
include Docker and a `trident-container.service` systemd unit.

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
- **Go 1.25+** (for building Go tools)
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

### 4. Build Trident RPMs

The **host** test images include Trident packages built from your local tree.
This step builds the RPMs into `bin/RPMS/`, which `testimages.py` passes to
Image Customizer via `--rpm-source`:

```bash
make bin/trident-rpms.tar.gz
```

This requires Docker and uses the Trident packaging Dockerfile to produce
RPMs from your compiled binary.

### 5. Build Trident Container Image (container runtime only)

For the **container** runtime, build a Docker image containing Trident and
export it as a tarball that netlaunch serves to the VM:

```bash
# Build the Docker image from the Trident RPMs
make docker-build

# Export as tarball to artifacts/test-image/
make artifacts/test-image/trident-container.tar.gz
```

This produces `trident/trident:latest` locally and saves it as a gzipped
tarball. The container installer ISO's management OS loads this image into
Docker on the target VM during install.

### 6. Generate SSH Keys

```bash
make artifacts/id_rsa
```

### 7. Download Base Image

```bash
# Downloads baremetal.vhdx from MCR
./tests/images/testimages.py download-image baremetal
```

### 8. Build COSI Images

Build the test COSI images that Trident will install and update. A/B updates
require two images with unique filesystem UUIDs — Trident rejects updates where
the new image matches the installed one. Use `--clones 2` to produce two images,
then rename them into `artifacts/test-image/`.

#### Host Runtime Images

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

#### Container Runtime Images

Container images include Docker and a `trident-container.service` but omit
Trident RPMs — Trident runs from the container tarball instead:

```bash
mkdir -p artifacts/test-image

# Regular container image
sudo ./tests/images/testimages.py build trident-container-testimage \
    --output-dir ./artifacts/test-image --clones 2
mv artifacts/test-image/trident-container-testimage_0.cosi artifacts/test-image/container.cosi
mv artifacts/test-image/trident-container-testimage_1.cosi artifacts/test-image/container_v2.cosi

# Verity container image (for root-verity configuration)
sudo ./tests/images/testimages.py build trident-container-verity-testimage \
    --output-dir ./artifacts/test-image --clones 2
mv artifacts/test-image/trident-container-verity-testimage_0.cosi artifacts/test-image/container-verity.cosi
mv artifacts/test-image/trident-container-verity-testimage_1.cosi artifacts/test-image/container-verity_v2.cosi

# UKI/usr-verity container image (for usr-verity, combined configurations)
sudo ./tests/images/testimages.py build trident-container-usrverity-testimage \
    --output-dir ./artifacts/test-image --clones 2
mv artifacts/test-image/trident-container-usrverity-testimage_0.cosi artifacts/test-image/container-usrverity.cosi
mv artifacts/test-image/trident-container-usrverity-testimage_1.cosi artifacts/test-image/container-usrverity_v2.cosi
```

The images use the Image Customizer container from
`mcr.microsoft.com/azurelinux/imagecustomizer:latest`.

### 9. Build the Installer ISO

The management OS ISO is used by `netlaunch` to boot the VM and run Trident.
Each runtime has its own installer ISO:

**Host runtime:**

```bash
make bin/trident-mos.iso
```

**Container runtime:**

The container installer ISO is not built locally — it is downloaded from the
pipeline:

```bash
make download-trident-container-installer-iso
```

This downloads the latest successful `trident-container-installer.iso` from the
CI pipeline to `artifacts/trident-container-installer.iso`. To download from a
specific pipeline run, set `RUN_ID`:

```bash
make download-trident-container-installer-iso RUN_ID=<run-id>
```

## Running a Clean Install

### 1. Create the QEMU VM

Use `virtdeploy` to create a VM with empty disks:

**Host runtime:**

```bash
sudo bin/virtdeploy create-one --disks 32,8
```

**Container runtime:**

Container scenarios require more memory (at least 11 GiB) because the VM must
run Docker alongside Trident:

```bash
sudo bin/virtdeploy create-one --disks 32,8 --mem 12
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

:::note Container runtime Host Configuration
For the **container** runtime, the Host Configuration must include an
`additionalFiles` entry to copy the Trident container tarball onto the VM.
Storm-trident and the pipeline's `edit_host_config.py` add this automatically
when `--runtimeEnv container` is specified:

```yaml
os:
  additionalFiles:
    - source: /var/lib/trident/trident-container.tar.gz
      destination: /var/lib/trident/trident-container.tar.gz
```

If running manually with the container runtime, add this entry to your Host
Configuration.
:::

### 3. Run netlaunch

**Host runtime:**

```bash
NETLAUNCH_PORT=4000 make run-netlaunch
```

**Container runtime:**

```bash
NETLAUNCH_PORT=4000 make run-netlaunch-container-images
```

This uses `artifacts/trident-container-installer.iso` as the installer ISO and
requires `artifacts/test-image/trident-container.tar.gz` to be present. The
container tarball is served alongside the COSI images so the management OS can
load it into Docker on the target VM.

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

For container runtime, change `-R host` to `-R container`:

```bash
cd tests/e2e_tests
python3 -u -m pytest -m daily --capture=no \
    -H <VM_IP> \
    -R container \
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

:::note SELinux checks
The `check-selinux` storm-trident helper only runs for the **host** runtime.
It is skipped for container scenarios.
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

For container runtime, change the fourth positional argument from `host` to
`container`:

```bash
sudo bin/storm-trident helper ab-update -- \
    artifacts/id_rsa \
    <VM_IP> \
    testing-user \
    container \
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

Test configurations live in `tests/e2e_tests/trident_configurations/`. Each
subdirectory defines a complete test scenario — what to install and which tests
to run against it.

### Directory Structure

```
tests/e2e_tests/trident_configurations/
├── base/
│   ├── trident-config.yaml    # Host Configuration for Trident
│   └── test-selection.yaml    # Which tests to run
├── combined/
├── encrypted-partition/
├── misc/
├── simple/
├── usr-verity/
└── ...
```

Each configuration directory contains two files:

- **`trident-config.yaml`**: The Host Configuration that Trident uses to
  provision the system. Defines disk layout, partitions, filesystems, A/B update
  volume pairs, users, SELinux mode, kernel parameters, and other OS settings.
  Image URLs use the placeholder `NETLAUNCH_HOST_ADDRESS`, which netlaunch
  replaces at runtime.
- **`test-selection.yaml`**: Declares which pytest test categories are
  compatible with this configuration, with optional per-ring overrides to
  add or remove tests at different pipeline stages.

### Test Selection

The `test-selection.yaml` file controls which tests run for a given
configuration. Each test file declares a pytest marker (e.g., `base`,
`encryption`, `verity`) via `pytestmark`, and the test selection's `compatible`
list references these markers.

**Available test markers:**

| Marker | Test File | What It Validates |
|--------|-----------|-------------------|
| `base` | `base_test.py` | Connection, partitions, users, UEFI boot entries |
| `encryption` | `encryption_test.py` | LUKS encrypted partitions and swap |
| `verity` | `verity_test.py` | dm-verity root or `/usr` integrity |
| `extensions` | `extensions_test.py` | System extension (sysext/confext) servicing |
| `rollback` | `rollback_test.py` | Health-check triggered rollback |
| `ab_update_staged` | `ab_update_staged_test.py` | Staged A/B update state |

**Basic test selection** — list the compatible markers:

```yaml
# tests/e2e_tests/trident_configurations/misc/test-selection.yaml
compatible:
  - base
```

This selects all tests marked `@pytest.mark.base` (from `base_test.py`).

**Multi-marker selection** — configurations that exercise multiple features
list all applicable markers:

```yaml
# tests/e2e_tests/trident_configurations/combined/test-selection.yaml
compatible:
  - base
  - usr_verity
  - encryption
  - uki
```

**Per-ring overrides** — test selections can be refined for each pipeline ring.
Rings are cumulative (each inherits from the previous), so overrides only need
to specify differences:

```yaml
compatible:
  - marker1
  - marker2
  - marker3
weekly:
  remove:
    - file2.py::function_name1    # Remove a specific test function
daily:
  remove:
    - marker3                      # Remove an entire marker category
post_merge:
  remove:
    - marker2
  add:
    - file2.py::function_name1    # Add back a specific test function
pullrequest:
  remove:
    - file2.py::function_name1
```

Overrides support both marker names (affecting all tests with that marker) and
specific test functions using `file.py::function_name` syntax.

### How Test Selection Works

The `conftest.py` `pytest_collection_modifyitems` hook processes
`test-selection.yaml` at collection time:

1. All markers listed in `compatible` are added to matching tests.
2. Ring-level overrides (`weekly`, `daily`, `post_merge`, `pullrequest`,
   `validation`) are applied cumulatively — each ring inherits the test set from
   the previous ring, then applies its own `add`/`remove` operations.
3. When pytest runs with `-m daily`, only tests that received the `daily` marker
   through this process are selected.

### Pipeline Scheduling

The file `tests/e2e_tests/target-configurations.yaml` maps each configuration
to the hardware types, runtimes, and pipeline frequencies where it should run:

```yaml
virtualMachine:
  host:
    pullrequest:        # Runs on every PR
      - base
      - misc
      - simple
      - ...
    post_merge:         # Runs after merge to main
      - base
      - misc
      - ...
    daily:              # Runs nightly
      - base
      - misc
      - ...
  container:
    pullrequest:
      - base
      - combined
      - ...
    post_merge:
      - base
      - combined
      - ...
    daily:
      - base
      - combined
      - ...
bareMetal:
  host:
    daily:
      - base
      - ...
  container:
    daily:
      - base
      - combined
      - ...
```

The `invert.py` script in `tools/storm/e2e/` transforms this file into
`configurations/configurations.yaml`, which storm-trident embeds at build time
for scenario discovery.

### Configuration Summary

| Configuration | Image | Key Features |
|--------------|-------|-------------|
| `base` | `regular.cosi` | Standard grub2 install, baseline validation |
| `simple` | `regular.cosi` | Minimal single-root partition layout |
| `misc` | `regular.cosi` | NTFS partition, kernel modules, extra services, kernel command line |
| `split` | `regular.cosi` | Separate `/boot` partition |
| `encrypted-partition` | `regular.cosi` | LUKS-encrypted root partition |
| `encrypted-raid` | `regular.cosi` | LUKS encryption with RAID |
| `encrypted-swap` | `regular.cosi` | LUKS-encrypted swap partition |
| `raid-small` | `regular.cosi` | RAID-1 mirrored root (small disks) |
| `raid-mirrored` | `regular.cosi` | RAID-1 mirrored root |
| `raid-big` | `regular.cosi` | RAID with large disks |
| `raid-resync-small` | `regular.cosi` | RAID resync behavior |
| `extensions` | `regular.cosi` | System extensions (sysext/confext) |
| `health-checks-install` | `regular.cosi` | Health-check and rollback on install |
| `root-verity` | `verity.cosi` | dm-verity protected root filesystem |
| `usr-verity` | `usrverity.cosi` | dm-verity `/usr`, UKI, systemd-boot |
| `combined` | `usrverity.cosi` | UKI + encryption + verity combined |
| `memory-constraint-combined` | `usrverity.cosi` | Combined features under memory pressure |
| `rerun` | `usrverity.cosi` | Re-run idempotency validation |
| `usr-verity-raid` | `usrverity.cosi` | UKI + verity with RAID |

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

E2E scenarios are organized into test rings that control how frequently they run.
The valid `--test-ring` values are:

- **pr-e2e**: Run on every pull request (innermost ring)
- **ci**: Run after merge to main (post-merge)
- **pre**: Run during pre-release validation
- **full-validation**: Run for release validation (outermost ring)

Rings are cumulative — all scenarios in inner rings also run when an outer ring
is executed.

:::note
The `tests/e2e_tests/target-configurations.yaml` file uses pipeline-frequency
labels (`pullrequest`, `post_merge`, `daily`, `weekly`) which `invert.py` maps
to the ring constants above: `pullrequest` → `pr-e2e`, `post_merge` → `ci`,
`daily`/`weekly` → `full-validation`.
:::

### How E2E Discovery Works

E2E scenario discovery automatically finds all configured Host Configurations
and determines when each should run. The key components:

- **Configuration definitions**: All Host Configurations live in
  `tests/e2e_tests/trident_configurations/`, and the mapping of which
  configurations run in which test rings is defined in
  `tests/e2e_tests/target-configurations.yaml`.
- **Discovery function**: `DiscoverTridentScenarios` in
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
