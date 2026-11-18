---
sidebar_position: 10
---

# Synthetic ACL Tests

`storm-trident run synthetic-acl` tests native Azure Container Linux (ACL)
adoption and A/B servicing in a disposable Linux QEMU VM. It supports a regular
Azure Linux kernel (`standard`) and the EFI Tree Loader/HvLoader boot chain
(`mshv`). These are synthetic test images, not production ACL releases or
nightly ACL images. They omit Ignition, BTRFS, `/etc` OverlayFS and signing.

Unlike [rollback tests](Rollback-Tests.md), this scenario uses its own
`base.qcow2` and `update.cosi` fixtures. It does not require extension images,
an installer ISO, `netlaunch`, or the `trident-acl-agent` service.

## What It Tests

Both variants run the same Go scenario:

| Checkpoint | Expected slot | Expected generation | Trident state |
|------------|---------------|---------------------|---------------|
| Initial boot | A | Base | Not queried |
| A/B update and commit | B | Update | `provisioned` |
| A/B rollback and commit | A | Base | `provisioned` |
| A/B update and commit | B | Update | `provisioned` |
| Another A/B update and commit | A | Update | `provisioned` |
| Further reboot | A | Update | `provisioned` |

Initial boot checks the native ACL image without initializing a Trident
datastore. The first update performs adoption. Each update and rollback
separates staging from finalization, then explicitly commits after the new
boot. Staged checks verify that the running slot and boot ID have not changed.
Updates use `internalParams.noTransition: true` and a controller-driven
shutdown/start. Native rollback restores a derived configuration without
that override, so its finalization reboots automatically.

Checks include the physical `/usr` partition, read-only dm-verity mapping,
filesystem UUID and root hash, generation marker, Trident binary provenance,
active volume, bootloader selection, retained slot files and committed fallback
files. MSHV additionally requires its kernel, a character device at `/dev/mshv`,
and the complete per-slot hypervisor boot bundle.

## Prerequisites

Run these commands on an x86-64 Linux host with sudo, working KVM/libvirt,
`virt-install`, OVMF, and the libvirt `default` network with IPv4 DHCP.
The host must be able to SSH to guests on that network.

For an Ubuntu worker, the VM packages are:

```bash
sudo apt-get install -y qemu-kvm qemu-utils libvirt-daemon-system \
    libvirt-clients virtinst ovmf
sudo virsh -c qemu:///system net-list --all
```

Start the `default` network with `sudo virsh -c qemu:///system net-start default`
if it is inactive. The standard VM uses 2 vCPUs and 2 GiB RAM. MSHV uses Q35,
4 vCPUs, 16 GiB RAM and host CPU passthrough with nested VMX/SVM. A VM used as
the test host must itself expose those virtualization capabilities. MSHV also
requires plain, non-Secure-Boot firmware at
`/usr/share/OVMF/OVMF_CODE_4M.fd` and `/usr/share/OVMF/OVMF_VARS_4M.fd`.
Both variants run with Secure Boot disabled.

Building the controller requires Go 1.25+, Python 3, OpenSSL, `protoc` 3.15+
and the Go protobuf plugins. See [Requirements](../Building/Dependencies.md)
for installation and PATH setup. Building images additionally requires Docker,
libguestfs (`guestfish`), RPM, binutils, OpenSSH and a plain Azure Linux 3.0
x86-64 base image. Building the MSHV loader requires Rust and authorized access
to the private EFI Tree Loader repository and any configured Cargo feed.

Run the following examples from the Trident repository root in one shell:

```bash
WORK="$(pwd)/artifacts/synthetic-acl-local-$(date +%Y%m%d-%H%M%S)"
mkdir -p "$WORK"
```

## Build the Controller

On a fresh checkout, generate the shared protobuf and test TLS dependencies
before building Storm:

```bash
export PATH="$(go env GOPATH)/bin:$PATH"
make bin/netlaunch
make bin/storm-trident
```

`netlaunch` is built only to generate those shared dependencies; this scenario
does not run it. Rebuild `bin/storm-trident` after changing the Go scenario.

## Obtain the Images

### Reuse Pipeline Artifacts

Download and extract the `synthetic-acl-standard` and/or `synthetic-acl-mshv`
artifact from a pipeline run that built these fixtures. Place each variant in
its own directory, for example `$WORK/standard` and `$WORK/mshv`.

Each directory must contain these nonempty files directly, without another
artifact-directory level:

```text
base.qcow2
update.cosi
metadata.json
manifest.json
id_rsa
```

Keep each artifact's files together. The scenario verifies COSI/metadata
checksums and binary provenance; `id_rsa` must match the key embedded in that
image. Protect the private key with `chmod 600`. These are test credentials,
not production SSH keys.

This is sufficient for controller-only changes. To test changes to Trident
itself, rebuild the images with RPMs from the desired revision: rebuilding
the Go controller does not replace `/usr/bin/trident` inside an existing image.

### Build Images Locally

Install the image-building dependencies and pull the same Image Customizer
digest used by the synthetic-image pipeline:

```bash
sudo apt-get install -y binutils libguestfs-tools rpm rpm2cpio cpio
IC=mcr.microsoft.com/azurelinux/imagecustomizer@sha256:db7bcb75be27cbab4face6dbaedd3b6e6c63c65c369f5763d81499ed843ccd44
docker pull "$IC"
```

Set `BASE_IMAGE` to a plain Azure Linux 3.0 x86-64 VHDX or QCOW2, not an
existing ACL release. For example, use the public `minimal-os` image described
in [Servicing Tests](Servicing-Tests.md#4-download-the-qemu_guest-base-image).
Record its version/digest when reproducing a particular pipeline run.

Build Azure Linux 3 RPMs using the Docker build, then collect them in a clean
directory. This build requires the usual Azure/Cargo feed authentication:

```bash
BASE_IMAGE=/absolute/path/to/azurelinux.vhdx
make DISTRO=azl3 bin/trident-rpms-azl3.tar.gz
mkdir -p "$WORK/rpms"
find bin/RPMS -type f -name '*azl3*.rpm' -exec cp -- '{}' "$WORK/rpms/" \;
ssh-keygen -q -t rsa -b 3072 -N '' -f "$WORK/id_rsa"
```

The RPM directory must contain exactly one x86-64 `trident` RPM. The helper
builds both generations from that RPM and records its binary hash.
Image configuration lives in `tests/images/synthetic-acl/standard.yaml` and
`mshv.yaml`, with shared OS settings in `common.yaml`. The helper stages these
files unchanged and supplies runtime keys, package lists and generation input.

#### Standard Variant

```bash
sudo env LIBGUESTFS_BACKEND=direct ./tests/images/prepare-synthetic-acl.py \
    --variant standard \
    --base-image "$BASE_IMAGE" \
    --rpm-dir "$WORK/rpms" \
    --ssh-public-key "$WORK/id_rsa.pub" \
    --ssh-private-key "$WORK/id_rsa" \
    --image-customizer "$IC" \
    --output-dir "$WORK/standard"
```

The output directory must be empty or absent. Failed builds retain diagnostics
under `.work/`; use a new output directory rather than mixing build attempts.

#### MSHV Variant

Use an authorized EFI Tree Loader checkout at
`bf02d4e3f4b9a4b61e838fd4d3705a50573d1ce9`, with the slot-relative
`tests/images/synthetic-acl/load_entries.json` profile. The pipeline uses
`ms-prod-1.97.1`; an explicit local Rust 1.97.1 toolchain avoids the source's
different default toolchain pin:

```bash
ETL_SOURCE=/absolute/path/to/efitreeloader
test "$(git -C "$ETL_SOURCE" rev-parse HEAD)" = bf02d4e3f4b9a4b61e838fd4d3705a50573d1ce9
cp tests/images/synthetic-acl/load_entries.json "$ETL_SOURCE/efitreeloader/load_entries.json"
rustup toolchain install 1.97.1 --profile minimal --target x86_64-unknown-uefi
(
    cd "$ETL_SOURCE"
    cargo +1.97.1 build --locked --target x86_64-unknown-uefi --release
)
PROFILE_SHA256="$(sha256sum tests/images/synthetic-acl/load_entries.json | cut -d ' ' -f 1)"
./tests/images/synthetic-acl/fetch-mshv-payload.py --output-dir "$WORK/mshv-payload"

sudo env LIBGUESTFS_BACKEND=direct ./tests/images/prepare-synthetic-acl.py \
    --variant mshv \
    --base-image "$BASE_IMAGE" \
    --rpm-dir "$WORK/rpms" \
    --ssh-public-key "$WORK/id_rsa.pub" \
    --ssh-private-key "$WORK/id_rsa" \
    --image-customizer "$IC" \
    --efitreeloader "$ETL_SOURCE/target/x86_64-unknown-uefi/release/efitreeloader.efi" \
    --mshv-payload "$WORK/mshv-payload" \
    --loader-provenance "bf02d4e3f4b9a4b61e838fd4d3705a50573d1ce9; profile-sha256=$PROFILE_SHA256" \
    --output-dir "$WORK/mshv"
```

The payload helper fetches hash-pinned official Azure Linux packages, including
their license files. It does not extract a nightly ACL image. See
`tests/images/synthetic-acl/README.md` for image layout and payload details.

## Run the Scenario

```bash
sudo chmod 600 "$WORK/standard/id_rsa"
sudo ./bin/storm-trident run synthetic-acl -- \
    --variant standard \
    --artifacts-dir "$WORK/standard" \
    --output-path "$WORK/results-standard"
```

For MSHV, use `--variant mshv`, `--artifacts-dir "$WORK/mshv"` and a distinct
`--output-path "$WORK/results-mshv"`; protect its `id_rsa` in the same way.
Do not pass the other scenarios' `--uki`, `--platform` or
`--ssh-private-key-path` flags. This scenario uses QEMU and the artifact's key.

| Flag | Description | Default |
|------|-------------|---------|
| `--variant` | `standard` or `mshv`; must match the manifest | `standard` |
| `--artifacts-dir` | Directory containing the five fixture files | Required |
| `--output-path` | Parent directory for private run directories | Required |
| `--vm-name` | Prefix for the uniquely named disposable VM | `synthetic-acl` |
| `--boot-timeout` | Maximum wait for each SSH-ready boot | `10m` |
| `--operation-timeout` | Maximum duration of a deployment or guest operation | `30m` |

A successful run reports `manual-ab-update: PASS`. The output directory must
differ from the artifact directory. Each invocation creates a unique
`run-<uuid>` directory and a private working disk; build inputs are unchanged.

## Results and Troubleshooting

Inspect the newest run directory with sudo when needed:

- `result.json`: pass/fail, error, VM name and artifact provenance.
- `controller.log`: guest commands, separate stdout/stderr and boot IDs.
- `serial-console.log` / `serial.log`: firmware and guest console output.
- `evidence/<phase>/`: measurements and Go snapshot manifests; no baseline
  Trident status is requested.
- `evidence/*-stage.log`, `*-finalize.log` and `*-commit.log`: captured Trident
  output. `guest-evidence.tar.gz` archives controller-side checkpoints even
  when the guest is no longer reachable.
- `booted.qcow2`: retained on failure for offline inspection, removed on
  success. The owned VM is cleaned up in either case.

For SSH timeouts, check the serial log, libvirt network and guest DHCP lease
before increasing `--boot-timeout`. A new boot ID, not merely an open SSH port,
is required after each operation. Missing generated Go packages or embedded
E2E configurations usually mean the controller build steps were skipped.

MSHV needs the complete loader chain to initialize the hypervisor: booting
only `kernel-mshv`, or using QEMU's host-side `-accel mshv`, is not a pass.
MSHV boot/reset behavior can depend on the host CPU, nested KVM and firmware.
A reset after `second.efi` before Linux and a hang after Linux requests reboot
are different failure boundaries; retain the serial log and working disk
rather than treating either as proof of a slot or verity failure.
