---
sidebar_position: 8
---

# Rollback Tests

Rollback tests validate manual rollback and runtime update workflows using
`storm-trident run rollback`. Like [servicing tests](Servicing-Tests.md),
they start from a pre-built VM image defined in
`tests/images/trident-vm-testimage/`, but focus specifically on the rollback
chain: A/B updates, runtime updates (sysexts and netplan), and rolling each
back in sequence.

## VM Image Types

The rollback scenario is typically run with verity-enabled images that support
extensions:

| Image | Bootloader | Integrity | UKI | Extensions | `--uki` flag |
|-------|-----------|-----------|-----|------------|--------------|
| `trident-vm-usr-verity-testimage` | systemd-boot | `/usr` verity | Yes | Sysexts supported | Required |
| `trident-vm-grub-verity-testimage` | grub2 | Root verity | No | Not supported | Not needed |

All image configs live under `tests/images/trident-vm-testimage/base/`. The
base image is `qemu_guest` (see
[Servicing Tests — Create the qemu\_guest Base Image](Servicing-Tests.md#4-create-the-qemu_guest-base-image)
for how to create it from the publicly available `baremetal` image on MCR).

**`trident-vm-usr-verity-testimage`** (recommended) uses systemd-boot with a
Unified Kernel Image (UKI) and `/usr` dm-verity. This is the only image type
that supports full extension testing. When using this image, you **must** pass
`--uki` to `storm-trident run rollback` so that the `prepare-qcow2` step
generates the correct Image Customizer config with `os.uki.mode: passthrough`.

**`trident-vm-grub-verity-testimage`** uses grub2 with root dm-verity. It does
not support extensions, so you must pass `--skip-extension-testing` when using
this image type.

## What It Tests

The rollback scenario exercises a multi-step update-and-rollback sequence:

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

## Prerequisites

- **Linux host** with root access
- **libvirt and QEMU** installed and configured
- **Docker** (for building images with Image Customizer)
- **[oras](https://oras.land/)** CLI (for downloading base images from MCR)
- **Go 1.24+** (for building Go tools)
- **Rust** (latest stable, for building Trident)

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

### 4. Create the qemu\_guest Base Image

Follow the instructions in
[Servicing Tests — Create the qemu\_guest Base Image](Servicing-Tests.md#4-create-the-qemu_guest-base-image)
to download the `baremetal` image from MCR and convert it to `qemu_guest.vhdx`
using Image Customizer.

### 5. Build Extension Images

The rollback chain tests three versions of sysext extensions:

```bash
mkdir -p artifacts
pushd ./artifacts
../bin/storm-trident script build-extension-images --build-sysexts --num-clones 3
popd
```

### 6. Build the VM Image

Choose an image type and build both COSI and QCOW2:

```bash
# For UKI with usr verity (recommended — full extension testing):
TEST_IMAGE_NAME="trident-vm-usr-verity-testimage"

# Alternative: grub with root verity (no extension testing):
# TEST_IMAGE_NAME="trident-vm-grub-verity-testimage"

# Clean any previous test images
sudo rm -f artifacts/trident-vm-*-testimage.qcow2 artifacts/trident-vm-*-testimage.cosi

# Build the COSI and QCOW2
sudo ./tests/images/testimages.py build $TEST_IMAGE_NAME --output-dir ./artifacts
make artifacts/$TEST_IMAGE_NAME.qcow2
```

## Running the Rollback Scenario

The rollback scenario requires root access for VM creation via `virt-install`:

```bash
# For UKI images (trident-vm-usr-verity-testimage):
sudo ./bin/storm-trident run rollback -w --verbose \
    --artifacts-dir ./artifacts/ \
    --output-path /tmp/rollback-output \
    --platform qemu \
    --ssh-private-key-path ./artifacts/id_rsa \
    --ssh-public-key-path ./artifacts/id_rsa.pub \
    --uki
```

```bash
# For grub-verity images (trident-vm-grub-verity-testimage):
sudo ./bin/storm-trident run rollback -w --verbose \
    --artifacts-dir ./artifacts/ \
    --output-path /tmp/rollback-output \
    --platform qemu \
    --ssh-private-key-path ./artifacts/id_rsa \
    --ssh-public-key-path ./artifacts/id_rsa.pub \
    --skip-extension-testing
```

:::warning
The `--uki` flag is **required** for UKI images. Without it, the `prepare-qcow2`
step will fail because Image Customizer requires explicit UKI handling
(`os.uki.mode`) when the base image contains Unified Kernel Images.
:::

### Flags

| Flag | Description | Default |
|------|-------------|---------|
| `--artifacts-dir` | Directory containing VM images, COSI, and extensions | `./artifacts` |
| `--output-path` | Output directory for logs | `./output` |
| `--platform` | `qemu` or `azure` | `qemu` |
| `--ssh-private-key-path` | Path to SSH private key | `~/.ssh/id_rsa` |
| `--ssh-public-key-path` | Path to SSH public key | `~/.ssh/id_rsa.pub` |
| `--uki` | Image uses UKI (adds `os.uki.mode: passthrough` to IC config) | `false` |
| `--skip-runtime-updates` | Skip runtime update testing | `false` |
| `--skip-manual-rollbacks` | Skip manual rollback testing | `false` |
| `--skip-extension-testing` | Skip extension (sysext) testing | `false` |
| `--skip-netplan-runtime-testing` | Skip netplan runtime update testing | `false` |
| `--force-cleanup` | Force VM cleanup on exit | `false` |

### Test Cases

The rollback scenario runs these test cases in order:

1. **prepare-qcow2** — Modifies the QCOW2 using Image Customizer to inject the
   v1 sysext extension and enable `systemd-sysext`. For UKI images, adds
   `os.uki.mode: passthrough` to preserve existing UKIs.
2. **deploy-vm** — Creates and boots a QEMU VM from the prepared QCOW2
3. **check-deployment** — Verifies the VM booted successfully and is accessible
   via SSH
4. **multi-rollback** — Runs the full update → runtime update → rollback chain
   described in [What It Tests](#what-it-tests)
5. **skip-to-ab-rollback** — Tests skipping runtime rollbacks and going directly
   to A/B rollback
6. **split-rollback** — Tests split-phase (stage then finalize) rollback
7. **collect-logs** — Fetches Trident logs from the VM via SSH
8. **cleanup-vm** — Destroys the QEMU VM
