#!/bin/bash
set -euo pipefail

. $(dirname $0)/common.sh

mkdir -p artifacts/update-a
mkdir -p artifacts/update-b

SUFFIX=""
if [ "$TEST_PLATFORM" == "azure" ]; then
    SUFFIX="-azure"
fi

make -C ../test-images build/trident-vm-verity$SUFFIX-testimage.vhd
cp ../test-images/build/trident-vm-verity$SUFFIX-testimage.vhd $ARTIFACTS/trident-vm-verity$SUFFIX-testimage.vhd

make -C ../test-images trident-vm-verity$SUFFIX-testimage
cp ../test-images/build/trident-vm-verity$SUFFIX-testimage/* $ARTIFACTS/update-a/

make -C ../test-images trident-vm-verity$SUFFIX-testimage
cp ../test-images/build/trident-vm-verity$SUFFIX-testimage/* $ARTIFACTS/update-b/