#!/bin/bash
set -euo pipefail

. $(dirname $0)/common.sh

mkdir -p artifacts/update-a
mkdir -p artifacts/update-b

SUFFIX=""
EXTENSION=qcow2
if [ "$TEST_PLATFORM" == "azure" ]; then
    SUFFIX="-azure"
    EXTENSION=vhd
fi
if [ "$FLAVOR" == "uki" ]; then
    SUFFIX="-uki"
fi

make -C ../test-images build/trident-vm-verity$SUFFIX-testimage.$EXTENSION
cp ../test-images/build/trident-vm-verity$SUFFIX-testimage.$EXTENSION $ARTIFACTS/trident-vm-verity$SUFFIX-testimage.$EXTENSION

make -C ../test-images trident-vm-verity$SUFFIX-testimage
mv -f ../test-images/build/trident-vm-verity$SUFFIX-testimage.cosi $ARTIFACTS/update-a/

make -C ../test-images trident-vm-verity$SUFFIX-testimage
cp ../test-images/build/trident-vm-verity$SUFFIX-testimage.cosi $ARTIFACTS/update-b/