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
`trident-vm-*-testimage.qcow2` in the artifacts directory. The pipeline-tested
image types are:

| Image | Bootloader | Integrity | UKI | Config File |
|-------|-----------|-----------|-----|-------------|
| `trident-vm-grub-verity-testimage` | grub2 | Root verity | No | `updateimg-grub-verity.yaml` |
| `trident-vm-usr-verity-testimage` | systemd-boot | `/usr` verity | Yes | `baseimg-usr-verity.yaml` |

All image configs live under `tests/images/trident-vm-testimage/base/`. The
base image is `qemu_guest`.

**`trident-vm-grub-verity-testimage`** uses grub2 with root dm-verity. The root
filesystem is read-only and integrity-protected, with `/var` on a separate
partition and an `/etc` overlay service for runtime state. It uses the
`updateimg-grub-verity.yaml` config which includes SSH access, network
configuration, and sudoers for the test user.

**`trident-vm-usr-verity-testimage`** uses systemd-boot with a Unified Kernel
Image (UKI) and `/usr` dm-verity. This is a preview feature
(`previewFeatures: uki`) that requires `ukify` on the build host. It uses the
`baseimg-usr-verity.yaml` config which defines the full runtime layout.

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
- **Go 1.24+** (for building Go tools)
- **Rust** (latest stable, for building Trident)

The `qemu_guest` base image is not publicly available on MCR. It is downloaded
from an internal Azure DevOps artifacts feed by the Makefile target
`$(QEMU_GUEST_IMAGE)`. You need `az` CLI configured with access to the
`mariner-org` ADO organization, or you can obtain the image from a pipeline
artifact.

See [Dependencies](Dependencies.md) for full build dependency details.

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

The `qemu_guest` base image is downloaded automatically by the QCOW2 Makefile
targets via `az artifacts universal download`. Ensure you have `az` CLI
configured:

```bash
az login
```

### 5. Build the VM Image

Choose an image type and build the QCOW2:

```bash
# For grub with root verity:
make artifacts/trident-vm-grub-verity-testimage.qcow2

# For UKI with usr verity (requires ukify on build host):
make artifacts/trident-vm-usr-verity-testimage.qcow2
```

### 6. Prepare Update Images

The update loop needs two distinct COSI images — Trident rejects updates where
the new image has the same filesystem UUIDs as the currently installed one.
Use `--clones 2` to build two images with unique UUIDs, then rename them to
the same filename in each directory (the update loop picks the filename from
`update-a` and applies it to both update configs):

```bash
mkdir -p artifacts/update-a artifacts/update-b

# Build two clones of the COSI image (produces _0 and _1 suffixed files)
sudo ./tests/images/testimages.py build trident-vm-grub-verity-testimage \
    --output-dir ./artifacts --clones 2

# Move the clones into the update directories with the same filename
mv artifacts/trident-vm-grub-verity-testimage_0.cosi \
    artifacts/update-a/trident-vm-grub-verity-testimage.cosi
mv artifacts/trident-vm-grub-verity-testimage_1.cosi \
    artifacts/update-b/trident-vm-grub-verity-testimage.cosi
```

:::note
Both the QCOW2 and COSI images must be built from the same base image. Trident
validates that the COSI's `VARIANT_ID` in `/etc/os-release` matches the host's.
A mismatch (e.g., `baremetal` COSI on a `qemu-guest` VM) will fail with
"Mismatched OS release VARIANT_ID".
:::

## Running the Servicing Scenario

The servicing scenario requires root access for VM creation via `virt-install`:

```bash
sudo bin/storm-trident run servicing -- \
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
