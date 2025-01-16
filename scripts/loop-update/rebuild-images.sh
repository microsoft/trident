#/bin/bash

set -euo pipefail

. $(dirname $0)/common.sh

mkdir -p artifacts/update-a
mkdir -p artifacts/update-b

make -C ../test-images build/trident-vm-verity-testimage.qcow2
cp ../test-images/build/trident-vm-verity-testimage.qcow2 $ARTIFACTS/trident-vm-verity-testimage.qcow2

make -C ../test-images trident-vm-verity-testimage
cp ../test-images/build/trident-vm-verity-testimage/* $ARTIFACTS/update-a/

make -C ../test-images trident-vm-verity-testimage
cp ../test-images/build/trident-vm-verity-testimage/* $ARTIFACTS/update-b/