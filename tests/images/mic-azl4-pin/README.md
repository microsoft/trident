# Pinned MIC source for AZL4 builds

Azure Linux 4.0 support landed on `microsoft/azurelinux-image-tools` `main`
in [PR #698](https://github.com/microsoft/azurelinux-image-tools/pull/698)
(merge commit `c582ba91...`, 2026-05-21). The latest released tag at the
time of this writing is v1.3.0 (predates that PR), so there is no
`mcr.microsoft.com/azurelinux/imagecustomizer:<ver>` container that knows
AZL4 yet.

This directory pins the MIC source at a known-working SHA on upstream
`main`. The CI side-task at
`.pipelines/templates/stages/build_image/build-pinned-mic.yml` clones the
upstream repo at that SHA, builds the `imagecustomizer` binary, and then
builds the container locally. The result is consumed by the AZL4 test
image build via `--container imagecustomizer:azl4-pinned`.

`PINNED.env` is the single source of truth and is sourced by both the
CI template and the local `build-pinned-mic.sh`.

## How to bump the pin

1. Update `MIC_PIN_SHA` in `PINNED.env` to the target commit on upstream `main`.
2. Re-run `tests/images/mic-azl4-pin/build-pinned-mic.sh` locally to
   validate the container still builds.
3. Re-run the AZL4 E2E (build the AZL4 testimg COSI with the new container,
   netlaunch-install it, smoke-test boot) before committing.

## How to test changes locally

```bash
# In a Trident checkout:
./tests/images/mic-azl4-pin/build-pinned-mic.sh
python3 tests/images/testimages.py build trident-vm-grub-testimage-azl4 \
    --container imagecustomizer:azl4-pinned \
    --output-dir artifacts/test-image -f --no-download
```

The AZL4 base VHDX must be staged at `artifacts/azl4_qemu_guest.vhdx`.

## When this directory goes away

When a released MIC container exists that includes AZL4 support, this
whole directory plus `.pipelines/templates/stages/build_image/build-pinned-mic.yml`
should be deleted, and the AZL4 build template switched back to
`mcr.microsoft.com/azurelinux/imagecustomizer:<ver>` like every other
distro variant.

If a local patch on top of the pinned SHA becomes necessary while we
wait for that release, drop a `*.patch` file in this directory and
restore the patch-apply loop in `build-pinned-mic.sh` and
`build-pinned-mic.yml` (see git history of this directory for the
pattern).
