---
sidebar_position: 7
---

# Servicing Tests

Servicing tests validate multi-update workflows on pre-built VM images using
`storm-trident run servicing`. Unlike [E2E tests](E2E-Tests.md) which use
`netlaunch` and an installer ISO for initial provisioning, servicing tests start
from a QCOW2 image that already has Trident and an OS installed, then run
repeated A/B updates with optional rollback testing.

The VM images are defined in `tests/images/trident-vm-testimage/` and built
with Image Customizer from the `qemu_guest` base image.

## VM Image Types

The servicing scenario expects a QCOW2 image matching the pattern
`trident-vm-*-testimage.qcow2` in the artifacts directory. Each image type
tests a different bootloader and integrity configuration:

| Image | Bootloader | Integrity | UKI | Config File | Notes |
|-------|-----------|-----------|-----|-------------|-------|
| `trident-vm-grub-testimage` | grub2 | None | No | `updateimg-grub.yaml` | Standard grub without verity |
| `trident-vm-grub-verity-testimage` | grub2 | Root verity | No | `updateimg-grub-verity.yaml` | Root filesystem is dm-verity protected; `/var` on separate partition |
| `trident-vm-root-verity-testimage` | systemd-boot | Root verity | Yes | `baseimg-root-verity.yaml` | UKI with root verity; requires `ukify` on build host |
| `trident-vm-usr-verity-testimage` | systemd-boot | `/usr` verity | Yes | `baseimg-usr-verity.yaml` | UKI with `/usr` verity; requires `ukify` on build host |
| `trident-vm-grub-testimage-arm64` | grub2 | None | No | `updateimg-grub.yaml` | ARM64 variant; uses `core_arm64` base image |
| `trident-vm-grub-verity-testimage-arm64` | grub2 | Root verity | No | `updateimg-grub-verity.yaml` | ARM64 variant with root verity |

All image configs live under `tests/images/trident-vm-testimage/base/`. The
base image for amd64 variants is `qemu_guest` (downloaded from MCR); arm64
variants use `core_arm64`.

:::info Azure-only image
`trident-vm-grub-verity-azure-testimage` uses the `core_selinux` base image
and `updateimg-grub-verity-azure.yaml`. It is designed for Azure VMs and is not
compatible with local QEMU testing.
:::

### Update Images vs Base Images

The config files follow two patterns:

- **`updateimg-*`** configs (grub, grub-verity) are update-oriented: they
  include SSH access, network configuration, sudoers, and a service override
  for the test user. These are used for the standard servicing update loop.
- **`baseimg-*`** configs (root-verity, usr-verity) define the full runtime
  layout with UKI, systemd-boot, and verity. These are used for verity-based
  servicing tests.

### COSI Update Images

During the update loop, the servicing scenario serves COSI files over HTTP
using `netlisten`. It expects COSI files in two directories within the
artifacts dir:

- `<artifacts-dir>/update-a/` — COSI image served on port 8000 (configurable
  via `--update-port-a`)
- `<artifacts-dir>/update-b/` — COSI image served on port 8001 (configurable
  via `--update-port-b`)

The update loop alternates between these two images across iterations.

## Prerequisites

- **Linux host** with root access
- **libvirt and QEMU** installed and configured
- **Docker** (for building images with Image Customizer)
- **[oras](https://oras.land/)** CLI (for downloading base images from MCR)
- **Go 1.24+** (for building Go tools)
- **Rust** (latest stable, for building Trident)

See [Dependencies](Dependencies.md) for full details.

## Building Dependencies

### 1. Build Trident and RPMs

```bash
make build
make bin/trident-rpms.tar.gz
```

### 2. Build Go Tools

```bash
make bin/storm-trident
make bin/netlisten
```

### 3. Generate SSH Keys

```bash
make artifacts/id_rsa
```

### 4. Download Base Image

```bash
# Downloads qemu_guest.vhdx from MCR
./tests/images/testimages.py download-image qemu_guest
```

### 5. Build the VM Image

Choose an image type and build both COSI and QCOW2:

```bash
# For standard grub (no verity):
make artifacts/trident-vm-grub-testimage.qcow2

# For grub with root verity:
make artifacts/trident-vm-grub-verity-testimage.qcow2

# For UKI with usr verity (requires ukify):
make artifacts/trident-vm-usr-verity-testimage.qcow2
```

The Makefile targets handle building both the QCOW2 (initial VM image) and
any required COSI update images.

### 6. Prepare Update Images

Place COSI files in the update directories:

```bash
mkdir -p artifacts/update-a artifacts/update-b

# Copy the COSI images for the update loop
# (use the COSI images that match your test image type)
cp artifacts/trident-vm-grub-testimage.cosi artifacts/update-a/
cp artifacts/trident-vm-grub-testimage.cosi artifacts/update-b/
```

## Running the Servicing Scenario

```bash
bin/storm-trident run servicing -- \
    --artifacts-dir ./artifacts \
    --output-path /tmp/servicing-output \
    --platform qemu \
    --ssh-private-key-path ./artifacts/id_rsa \
    --verbose
```

### Test Cases

The servicing scenario runs these test cases in order:

1. **publish-sig-image** — Publishes the image to Azure SIG (skipped for QEMU)
2. **deploy-vm** — Finds a `trident-vm-*-testimage.qcow2` in artifacts, copies
   it to `booted.qcow2`, and creates a QEMU VM
3. **check-deployment** — Verifies the VM booted and is accessible via SSH
4. **update-loop** — Runs repeated A/B updates: starts `netlisten` servers on
   ports 8000/8001 to serve COSI images, SSHes into the VM, edits the Host
   Configuration, and triggers `trident grpc-client update` with stage then
   finalize operations
5. **rollback** — Tests rollback after update (only when `--rollback` is enabled)
6. **collect-logs** — Fetches Trident logs from the VM via SSH
7. **cleanup-vm** — Destroys the QEMU VM

### Flags

| Flag | Description | Default |
|------|-------------|---------|
| `--artifacts-dir` | Directory containing VM images and COSI files | `/tmp` |
| `--output-path` | Output directory for logs | `./output` |
| `--platform` | `qemu` or `azure` | `qemu` |
| `--ssh-private-key-path` | Path to SSH private key | `~/.ssh/id_rsa` |
| `--user` | SSH user on the VM | `testuser` |
| `--rollback` | Enable rollback testing | `false` |
| `--retry-count` | Number of update retry attempts | `3` |
| `--update-port-a` | Port for first update server | `8000` |
| `--update-port-b` | Port for second update server | `8001` |
| `--verbose` | Enable verbose logging | `false` |
| `--force-cleanup` | Force VM cleanup on exit | `false` |
| `--test-case-to-run` | Run a specific test case only | `all` |
