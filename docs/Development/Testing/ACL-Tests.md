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
They request Trident from the pipeline's AZL3 RPMs; MSHV also replaces the
kernel and installs its complete boot chain. They preserve ACL's root-SSH
denial, using `testuser` with passwordless sudo instead. Images are unsigned;
Secure Boot is disabled. A passing run does not validate production signing.

The base QCOW2 and update COSI are separate normal `testimages.py` builds,
using the same SSH key and requested Trident RPM inputs. The scenario follows
active slots rather than matching filesystem UUIDs, verity root hashes or
Trident binary digests.

The scenario requires:

1. Baseline boot on USR-A with read-only, verity-backed `/usr`.
2. Update to B, rollback to A, update to B, then update to A.
3. Explicit staging/finalization and commit at each operation, with Provisioned
   status on volumes B/A/B/A and another persistence reboot.

It checks running boot IDs, active slot partitions, native ACL identity,
boot-tree preservation and bootctl selection. MSHV additionally requires its
kernel, character device `/dev/mshv` and complete slot-owned loader bundle.
The controller retains checkpoints across guest reboots and test cases.

Storm reports `deploy-vm`, `check-deployment`, `run-ab-update`, `run-rollback`,
`run-ab-update-2`, `run-ab-update-3`, `check-persistence`, `collect-logs` and
`cleanup-vm` separately. These are ordered cases sharing one disposable VM.
A failed case stops later cases. The failure hook collects diagnostics before
any cleanup pause, and the cleanup hook still removes the VM when its named
case was skipped. Collection and cleanup are not repeated after their named
cases succeed; failed working disks remain available for diagnosis.

Controller-requested standard reboots invoke `sudo -n systemctl reboot --no-block`
over SSH. MSHV instead invokes `sudo -n systemctl poweroff --no-block`, waits
for libvirt shutoff, then starts the existing domain without replacing its disk
or NVRAM. Shutoff is polled using the boot timeout without a forced-poweroff
fallback. Serial output is retained and the guest address is rediscovered.
Rollback finalization requests its own reboot through Trident. For MSHV only,
the controller requires Trident's reboot announcement and a fresh Linux
`reboot: Restarting system` serial message before stopping and starting the
existing libvirt domain. It monitors serial concurrently with finalization so
a stalled warm reset cannot leave SSH waiting indefinitely. Missing evidence,
command failures and timeouts fail the test without a host cold cycle.
The shutdown excerpt is saved before domain start can truncate the serial log.
This exercises MSHV rollback with a host cold cycle, not working warm reset.
The next checkpoint still requires a new boot ID, working SSH, explicit commit
and the expected slot before servicing continues.

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

Packaging verifies native ACL identity and compressed payload hashes, but not
filesystem UUIDs, verity root hashes or Trident binary digests. The disposable
test's update configuration uses `image.sha384: ignored`; metadata-digest and
whole-COSI checksum checks are outside this scenario. Its MSHV-only metadata
correction works around the pinned IC's hardcoded ACL bootloader tag; it neither
rebuilds partitions nor rewrites UKIs. Do not use it to relabel arbitrary production
COSIs.

## Pipelines

`testing_acl` builds and tests both variants in the four existing PR-e2e,
CI, prerelease and full-validation entrypoints, which explicitly set
`includeAclTests: true`. Other shared-template callers default to no ACL tests.
Jobs use the Linux KVM pool without Windows/Hyper-V. `ResolveAclTemplate` downloads
ACL-T once from pipeline **5304**: latest completed/successful **scheduled** run,
`acl-pipelines` at `main` and resolved `azure_container_linux` at `aclmain`.
Both image jobs consume the same ACL-T image artifact, build the pinned IC
revision and invoke ordinary `testimages.py` targets with the pipeline Trident
RPMs and SSH keys. MSHV additionally downloads the pinned producer efitreeloader
artifact and uses `overlay.index=off` to permit switching `/usr` lower layers
with persistent `/etc` OverlayFS state. Generic pipeline MIC version parameters
do not override the ACL-enabled IC pin.
The pipeline build service needs read access to ACL pipeline runs and artifacts
and authorization for the `aclImageTools` repository resource.

Published inputs are `acl-standard` and `acl-mshv`, each containing
`base.qcow2`, `update.cosi`, `metadata.json`, `manifest.json` and `id_rsa`.
The Storm scenario requires only `base.qcow2`, `update.cosi` and `id_rsa`;
it does not read the standalone metadata, input manifest or COSI contents.
The private key must remain restricted to the test artifact.

Results are `acl-standard-results` and `acl-mshv-results`. Each run preserves
the controller log, serial console, command output and snapshots,
plus `result.json` and `guest-evidence.tar.gz`. Failed VM working
disks are retained after VM cleanup; input images are not modified.
QEMU creates `booted.qcow2` in the artifacts directory; the scenario removes it
after a successful run. Evidence is written directly to `--output-path`, and the VM
uses `--vm-name` without a generated suffix. Runs must not share a VM name,
artifacts directory or output path concurrently.

## Validation limits

Initial native QCOW2/COSI builds, content audits and the complete standard
servicing cycle with explicit commits and persistence reboot passed using
existing Trident artifact 1256724 (`3fc019c`), not a fresh source-branch RPM.
An earlier local MSHV boot reset before Linux after `second.efi`.

[Pipeline 1262480, MSHV job](https://dev.azure.com/mariner-org/ECF/_build/results?buildId=1262480&view=logs)
at source `a869fe163` subsequently passed update, rollback, update, update,
four explicit commits on B/A/B/A and a persistence boot on A. This demonstrates
the native MSHV servicing scenario with the host cold cycles described above,
not every job in that pipeline or guest warm-reset support.
Secure Boot, production signing and cross-version hypervisor/loader servicing
are not validated. The pinned loader still uses shared absolute component paths;
same-generation bundle checks do not establish cross-version slot isolation.
