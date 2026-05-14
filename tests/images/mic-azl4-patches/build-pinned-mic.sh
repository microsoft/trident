#!/bin/bash
# TODO: drop this script once AZL4 ships in a released MIC container.
# See tests/images/mic-azl4-patches/README.md for the unpinning playbook.
#
# Builds the pinned MIC container locally and runs a smoke test against
# the AZL4 base VHDX expected at artifacts/azl4_qemu_guest.vhdx. Intended
# for developers iterating on the AZL4 test image without spinning up CI.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TRIDENT_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"

# Source of truth for the pin — keep this in sync with
# .pipelines/templates/stages/build_image/build-pinned-mic.yml.
PIN_URL="${MIC_PIN_URL:-https://github.com/microsoft/azurelinux-image-tools.git}"
PIN_SHA="${MIC_PIN_SHA:-9b7f9806b8c8a0e7c9bea0b69f4d4a4f9e5c1e23}"
CONTAINER_TAG="${MIC_CONTAINER_TAG:-imagecustomizer:azl4-pinned}"

WORKDIR="${WORKDIR:-$(mktemp -d)}"
echo "[pinned-mic] workdir: $WORKDIR"

git clone --filter=blob:none "$PIN_URL" "$WORKDIR"
cd "$WORKDIR"
git checkout "$PIN_SHA"

for patch in "$SCRIPT_DIR"/*.patch; do
    echo "[pinned-mic] applying $(basename "$patch")"
    git apply --check "$patch"
    git apply "$patch"
done

echo "[pinned-mic] building imagecustomizer binary"
( cd toolkit/tools/imagecustomizer && go build -o ../../out/tools/imagecustomizer . )

# TODO: drop this once the upstream container build script stops requiring
# pre-staged LICENSES. (Tracked in azurelinux-image-tools.)
mkdir -p toolkit/out/LICENSES
cp LICENSE toolkit/out/LICENSES/ 2>/dev/null || true

echo "[pinned-mic] building container $CONTAINER_TAG"
DOCKER_BUILDKIT=1 ./toolkit/tools/imagecustomizer/container/build-container.sh \
    -t "$CONTAINER_TAG" -a amd64

echo "[pinned-mic] building osmodifier binary"
mkdir -p "$TRIDENT_ROOT/tests/images/trident-vm-testimage/base/osmodifier-bin"
( cd toolkit/tools/osmodifier && go build \
    -o "$TRIDENT_ROOT/tests/images/trident-vm-testimage/base/osmodifier-bin/osmodifier" . )

echo "[pinned-mic] done."
echo "Container: $CONTAINER_TAG"
echo "osmodifier: $TRIDENT_ROOT/tests/images/trident-vm-testimage/base/osmodifier-bin/osmodifier"
