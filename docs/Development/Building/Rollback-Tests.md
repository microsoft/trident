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

The rollback scenario supports the same VM image types as
[servicing tests](Servicing-Tests.md#vm-image-types), but is typically run
with verity-enabled images that support extensions:

| Image | Bootloader | Extensions | Use Case |
|-------|-----------|------------|----------|
| `trident-vm-usr-verity-testimage` | systemd-boot (UKI) | Sysexts supported | **Recommended** — tests full rollback chain including extensions |
| `trident-vm-grub-verity-testimage` | grub2 | Not supported | Tests rollback without extensions (use `--skip-extension-testing`) |

All image configs live under `tests/images/trident-vm-testimage/base/`. See
[Servicing Tests](Servicing-Tests.md#vm-image-types) for the complete image
type reference.

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

See [Dependencies](Dependencies.md) for full details.

## Building Dependencies

```bash
# Choose the test image
TEST_IMAGE_NAME="trident-vm-usr-verity-testimage"
# Alternative: TEST_IMAGE_NAME="trident-vm-grub-verity-testimage"

# Build storm-trident
make bin/storm-trident

# Build the test sysext extension images (3 versions for rollback chain)
pushd ./artifacts
../bin/storm-trident script build-extension-images --build-sysexts --num-clones 3
popd

# Build Trident RPMs (baked into the VM image)
sudo rm -f bin/trident-rpms.tar.gz
sudo rm -rf bin/RPMS
make bin/trident-rpms.tar.gz

# Clean any previous test images
sudo rm -f artifacts/trident-vm-*-testimage.qcow2 artifacts/trident-vm-*-testimage.cosi

# Generate SSH keys (needed by the QCOW2 build)
make artifacts/id_rsa

# Download base image from MCR
./tests/images/testimages.py download-image qemu_guest

# Build the required test images (COSI + QCOW2)
make artifacts/$TEST_IMAGE_NAME.cosi
make artifacts/$TEST_IMAGE_NAME.qcow2
```

## Running Locally

```bash
sudo ./bin/storm-trident run rollback -w --verbose \
    --artifacts-dir ./artifacts/ \
    --output-path /tmp/output \
    --platform qemu \
    --ssh-private-key-path ./artifacts/id_rsa \
    --ssh-public-key-path ./artifacts/id_rsa.pub
```

### Skip Flags

Individual parts of the rollback chain can be skipped:

| Flag | Description |
|------|-------------|
| `--skip-runtime-updates` | Skip runtime update testing |
| `--skip-manual-rollbacks` | Skip manual rollback testing |
| `--skip-extension-testing` | Skip extension (sysext) testing |
| `--skip-netplan-runtime-testing` | Skip netplan runtime update testing |

:::warning
When using `trident-vm-grub-verity-testimage`, add `--skip-extension-testing`
since Image Customizer cannot add extensions to the original QCOW2 for that
image type.
:::

### Test Cases

The rollback scenario runs these test cases:

1. **prepare-qcow2** — Modifies the QCOW2 to include the v1 sysext extension
2. **deploy-vm** — Creates and boots a QEMU VM from the prepared QCOW2
3. **check-deployment** — Verifies the VM booted successfully
4. **multi-rollback** — Runs the full update → runtime update → rollback chain
5. **skip-to-ab-rollback** — Tests skipping runtime rollbacks and going directly
   to A/B rollback
6. **split-rollback** — Tests split-phase (stage then finalize) rollback
7. **collect-logs** — Fetches Trident logs from the VM
8. **cleanup-vm** — Destroys the QEMU VM
