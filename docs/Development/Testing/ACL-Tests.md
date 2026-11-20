---
title: ACL Tests
parent: Testing
---
# ACL Tests

`storm-trident run acl` exercises native Azure Container Linux adoption and
manual A/B servicing in disposable Linux/QEMU VMs. This guide covers running
the tests with images that have already been created, for both regular-kernel
and MSHV variants.

## Image and test contract

For standard (non-MSHV) variant, use an ACL image. The `mshv` variant
requires ACL images with the MSHV kernel and complete boot chain.
MSHV images are not publicly available.

Provide an already-created base QCOW2 and update COSI for the selected variant,
plus the private SSH key authorized in both images:

| File | Requirement |
|------|-------------|
| `base.qcow2` | Bootable ACL base image, initially on slot A |
| `update.cosi` | Compatible ACL update image for the selected variant |
| `id_rsa` | Matching private SSH key, restricted to mode 0600 |

Both images must provide Trident and public-key SSH access as `testuser`
with passwordless sudo. ACL's root-SSH denial can remain enabled.
Secure Boot is disabled for these disposable VMs; a passing run does not
validate production signing.

The scenario follows active slots rather than matching filesystem UUIDs,
verity root hashes or Trident binary digests.

The scenario requires:

1. Baseline boot and SSH readiness.
2. Update to B, rollback to A, update to B, then update to A.
3. Explicit staging/finalization and commit at each operation, with Provisioned
   status on volumes B/A/B/A and another persistence reboot.

After each commit and the persistence reboot, it checks that Trident status
reports `servicingState: provisioned` and the expected `abActiveVolume`.
Boot IDs ensure reboot waits observe a new boot.

Storm reports `deploy-vm`, `check-deployment`, `run-ab-update`, `run-rollback`,
`run-ab-update-2`, `run-ab-update-3`, `check-persistence`, `collect-logs` and
`cleanup-vm` separately. These are ordered cases sharing one disposable VM.
The first update case creates/uploads its host configuration and uploads the
COSI; later updates reuse those files. Deployment only starts the VM and waits for SSH.
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

Run from the Trident root on a KVM/libvirt-capable Linux host with QEMU,
`virt-install`, OVMF firmware and the Storm build prerequisites described in
[Building](../Building/Building.md). MSHV additionally requires host VMX/SVM support
with nested KVM enabled; its VM uses four CPUs, 16 GiB RAM and the plain OVMF
4M CODE/VARS firmware.

These tests have only been run on Intel platforms. MSHV tests may have
compatibility issues on AMD platforms.

Copy the already-created images and their matching key into a dedicated
artifacts directory using the filenames below. Do not generate a new SSH key:
it would not match the key authorized in the images.

```bash
WORK="$(pwd)/artifacts/acl-local-$(date +%Y%m%d-%H%M%S)"
mkdir -p "$WORK/inputs"
cp --reflink=auto /path/to/acl-base.qcow2 "$WORK/inputs/base.qcow2"
cp /path/to/acl-update.cosi "$WORK/inputs/update.cosi"
install -m 0600 /path/to/matching-private-key "$WORK/inputs/id_rsa"
VARIANT=standard                  # use mshv for MSHV
make bin/storm-trident
sudo ./bin/storm-trident run acl -a -- --variant "$VARIANT" \
  --artifacts-dir "$WORK/inputs" --output-path "$WORK/results" \
  --vm-name "acl-$VARIANT-$(date +%s)"
```

If the input directory already contains `base.qcow2`, `update.cosi` and
`id_rsa`, pass it directly with `--artifacts-dir` instead of copying the files.
Use `--boot-timeout` (default `10m`) and `--operation-timeout` (default `30m`)
to adjust the per-boot and per-operation limits.

The disposable test's update configuration uses `image.sha384: ignored`.
The scenario does not inspect COSI contents or validate artifact checksums,
filesystem UUIDs, verity root hashes or Trident binary digests.

## Results and diagnostics

Results are `acl-standard-results` and `acl-mshv-results`. Each run preserves
the controller log, serial console, command output and captured Trident status,
plus `result.json` and `guest-evidence.tar.gz`. Failed VM working
disks are retained after VM cleanup; input images are not modified.
QEMU creates `booted.qcow2` in the artifacts directory; the scenario removes it
after a successful run. Evidence is written directly to `--output-path`, and the VM
uses `--vm-name` without a generated suffix. Runs must not share a VM name,
artifacts directory or output path concurrently.
