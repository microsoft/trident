# Pinned MIC source for AZL4 builds

Azure Linux 4.0 support landed on `microsoft/azurelinux-image-tools` `main`
in [PR #698](https://github.com/microsoft/azurelinux-image-tools/pull/698)
(merge commit `c582ba91...`, 2026-05-21). There is not yet a released
`mcr.microsoft.com/azurelinux/imagecustomizer` container that includes it
(latest tag at the time of this writing is v1.3.0), so the Trident CI
pipeline still can't build the AZL4 test image with the standard `:latest`
container.

This directory pins the MIC source at a known-working SHA. The CI side-task
at `.pipelines/templates/stages/build_image/build-pinned-mic.yml` clones the
upstream repo at the pinned SHA, applies any local patches in this directory
(currently none), builds the `imagecustomizer` binary, and then builds the
container locally. The result is consumed by the AZL4 test image build via
`--container imagecustomizer:azl4-pinned`.

## How to bump the pin

1. Update `MIC_PIN_SHA` in `PINNED.env` to the target commit on `main`.
   `PINNED.env` is the single source of truth for the pin and is sourced by
   both `.pipelines/templates/stages/build_image/build-pinned-mic.yml`
   and the local `build-pinned-mic.sh`.
2. Delete every patch in this directory that's already covered by the new pin.
   Each patch's commit message points at the upstream PR it carries.
3. Eventually, when AZL4 ships in a released MIC container, delete this whole
   directory plus `build-pinned-mic.yml` and switch the AZL4 build template
   back to the standard `mcr.microsoft.com/azurelinux/imagecustomizer:<ver>`
   container.

## Patches

None currently. The previous patch (`0001-dnf-logdir-fix.patch`) is
superseded by [#698](https://github.com/microsoft/azurelinux-image-tools/pull/698)
which replaced the `dnf info --installed` call with `rpm -q` upstream,
solving the same read-only-logdir bug.

## Why a patch set instead of a fork

Holding a personal fork of `azurelinux-image-tools` means we're on the hook to
keep rebasing it. A patch set against a pinned SHA is friction-free as long as
patches stay small and stable, which has held so far (we've carried zero to
one one-liner at a time). If we end up carrying more than ~5 patches or
anything invasive, fork.

## How to test changes locally

```bash
# In a Trident checkout:
./tests/images/mic-azl4-patches/test-locally.sh
```

The script clones the pinned MIC, applies the patches, builds the binary +
container, then runs `testimages.py build trident-vm-grub-testimage-azl4`
against a local AZL4 base VHDX (must be staged at `artifacts/azl4_qemu_guest.vhdx`).
