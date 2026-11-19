---
title: ACL Tests
parent: Testing
---
# ACL Tests

`storm-trident run acl` exercises native Azure Container Linux adoption and
manual A/B servicing in disposable Linux/QEMU VMs. It runs both regular-kernel
and MSHV variants built from the same real ACL-T source as `acl-baremetal-devel`.

## Image and test contract

The static recipes in [`tests/images/acl`](../../../tests/images/acl/README.md)
retain ACL's Btrfs `/usr`, verity, `/etc` OverlayFS and native partition layout.
They replace Trident from the pipeline's AZL3 RPMs; MSHV also replaces the
kernel and installs its complete boot chain. They preserve ACL's root-SSH
denial, using `testuser` with passwordless sudo instead. Images are unsigned;
Secure Boot is disabled. A passing run does not validate production signing.

The base QCOW2 and update COSI are separate normal `testimages.py` builds,
with different `/usr` filesystem UUIDs and the same SSH key and Trident binary.
Their actual UUIDs and verity hashes identify generations; no artificial
os-release identity or generation marker is added.

The scenario requires:

1. Baseline boot on USR-A with read-only verified `/usr` and the candidate Trident.
2. Update to B, rollback to A, update to B, then update to A.
3. Explicit staging/finalization and commit at each operation, with Provisioned
   status on volumes B/A/B/A and another persistence reboot.

It checks running boot IDs, `/usr` UUID/hash, native ACL identity, Trident hash,
boot-tree preservation and bootctl selection. MSHV additionally requires its
kernel, character device `/dev/mshv` and complete slot-owned loader bundle.
The controller retains checkpoints across guest reboots.
Controller-requested standard reboots invoke `sudo -n systemctl reboot --no-block`
over SSH. MSHV instead invokes `sudo -n systemctl poweroff --no-block`, waits
for libvirt shutoff, then starts the existing domain without replacing its disk
or NVRAM. Shutdown is bounded by the boot timeout; it never falls back to forced
poweroff. Serial output is retained and the guest address is rediscovered.
Rollback finalization still requests its own reboot through Trident; that warm
reboot is not redirected by this controller workaround. The next checkpoint
requires a new boot ID and a working SSH connection before servicing continues.

## Local execution

See the image README for ACL-T selection and the pinned efitreeloader/ACL-enabled IC
inputs. Stage AZL3 candidate RPMs in `bin/RPMS` and build `acl-ic:local`.
The existing `requires_trident` setting supplies those RPMs; both recipes
remove/install Trident and retain the base repositories for dependencies.
Run from the Trident root on a KVM/libvirt-capable Linux host. Artifact packaging
also needs `libguestfs-tools`, `qemu-utils`, `jq` and `zstd`.

```bash
python3 tests/images/testimages.py download-image acl-t
WORK="$(pwd)/artifacts/acl-local-$(date +%Y%m%d-%H%M%S)"
mkdir -p "$WORK"
ssh-keygen -q -t rsa -b 2048 -N '' -f "$WORK/id_rsa"
IMAGE=acl                         # use acl-mshv for MSHV
VARIANT=standard                  # use mshv for MSHV
for PHASE in base update; do
  TARGET="$IMAGE"
  if [[ "$PHASE" == update ]]; then TARGET="$IMAGE-update"; fi
  cp "$WORK/id_rsa.pub" tests/images/acl/files/id_rsa.pub
  python3 tests/images/testimages.py build "$TARGET" --container acl-ic:local \
    --output-dir "$WORK/$PHASE"
done
sudo env LIBGUESTFS_BACKEND=direct tests/images/acl/publish-servicing-artifacts.sh \
  "$VARIANT" "$WORK/base/$IMAGE.qcow2" "$WORK/update/$IMAGE-update.cosi" \
  "$WORK/id_rsa" "$WORK/published"
make bin/storm-trident
sudo ./bin/storm-trident run acl -a -- --variant "$VARIANT" \
  --artifacts-dir "$WORK/published" --output-path "$WORK/results"
```

Refresh ACL-T once before building all variants.

Packaging verifies matching candidate binaries, native ACL identity, distinct
`/usr` UUIDs and compressed payload hashes. Its MSHV-only metadata correction
works around the pinned IC's hardcoded ACL bootloader tag; it neither rebuilds
partitions nor rewrites UKIs. Do not use it to relabel arbitrary production COSIs.

## Pipelines

`testing_acl` builds and tests both variants in the four existing PR-e2e,
CI, prerelease and full-validation entrypoints. Jobs use the Linux KVM pool;
there is no Windows/Hyper-V dependency. `ResolveAclTemplate` selects and downloads
ACL-T once from pipeline **5304**: latest completed/successful **scheduled** run,
`acl-pipelines` at `main` and resolved `azure_container_linux` at `aclmain`.
Both image jobs consume the same ACL-T image artifact. They download the
pinned producer efitreeloader artifact, build the pinned IC revision,
and invoke ordinary `testimages.py` targets with the pipeline Trident RPMs
and SSH keys. Generic pipeline MIC version parameters do not override the
ACL-enabled IC pin.
The pipeline build service needs read access to ACL pipeline runs and artifacts
and authorization for the `aclImageTools` repository resource.

Published inputs are `acl-standard` and `acl-mshv`, each containing
`base.qcow2`, `update.cosi`, `metadata.json`, `manifest.json` and `id_rsa`.
The private key must remain restricted to the test artifact.

Results are `acl-standard-results` and `acl-mshv-results`. Each run preserves
the controller log, serial console, command output, snapshots, metadata and
provenance, plus `result.json` and `guest-evidence.tar.gz`. Failed VM working
disks are retained after VM cleanup; immutable input images are not modified.

## Validation limits

Native QCOW2/COSI builds, content audits and the complete standard servicing
cycle with explicit commits and persistence reboot passed using existing Trident
artifact 1256724 (`3fc019c`), not a fresh RPM from this source branch.
MSHV builds and offline audits passed, but local
QEMU resets before Linux after `second.efi`. Prior pipeline reboot/shutdown
failures are a separate issue. Do not infer a native MSHV servicing pass.
