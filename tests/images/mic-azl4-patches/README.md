# Pinned MIC source for AZL4 builds

Azure Linux 4.0 support in the Image Customizer (MIC) lives across a stack of
open PRs in `microsoft/azurelinux-image-tools`. None of them are merged yet,
and there is no released `mcr.microsoft.com/azurelinux/imagecustomizer`
container that knows AZL4. So the Trident CI pipeline can't build the AZL4
test image with the standard `:latest` container.

This directory pins the MIC source at a known-working SHA + a local patch set.
The CI side-task at
`.pipelines/templates/stages/build_image/build-pinned-mic.yml` clones the
upstream repo at the pinned SHA, applies these patches, builds the
`imagecustomizer` binary, and then builds the container locally. The result
is consumed by the AZL4 test image build via `--container imagecustomizer:azl4-pinned`.

## How to bump the pin

When Vince merges his stack (or any of the patches below is upstream):

1. Update `pinnedSha` in `.pipelines/templates/stages/build_image/build-pinned-mic.yml`
   to the merge commit on `main`.
2. Delete every patch in this directory that's already covered by the new pin.
   Each patch's commit message points at the upstream PR it carries.
3. Eventually, when AZL4 ships in a released MIC container, delete this whole
   directory plus `build-pinned-mic.yml` and switch the AZL4 build template
   back to the standard `mcr.microsoft.com/azurelinux/imagecustomizer:<ver>`
   container.

## Patches

| File | Carries | Upstream PR |
|---|---|---|
| `0001-dnf-logdir-fix.patch` | `dnf info --setopt=logdir=/tmp` so package detection works on the read-only post-resize rootfs | [microsoft/azurelinux-image-tools#698](https://github.com/microsoft/azurelinux-image-tools/pull/698) |

## Why a patch set instead of a fork

Holding a personal fork of `azurelinux-image-tools` means we're on the hook to
keep rebasing it. A patch set against a pinned SHA is friction-free as long as
patches are small and stable — which they are today (one upstreamable
one-liner). If we end up carrying more than ~5 patches or anything invasive,
fork.

## How to test changes locally

```bash
# In a Trident checkout:
./tests/images/mic-azl4-patches/test-locally.sh
```

The script clones the pinned MIC, applies the patches, builds the binary +
container, then runs `testimages.py build trident-vm-grub-testimage-azl4`
against a local AZL4 base VHDX (must be staged at `artifacts/azl4_qemu_guest.vhdx`).
