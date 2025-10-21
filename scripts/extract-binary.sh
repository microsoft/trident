#!/bin/bash

# Takes a path to a directory containing a Trident RPM and extracts the Trident
# binary to the specified path.

set -eux

TMP_DIR=$(mktemp -d)
RPM=$(find $1 | grep -P 'trident-\d.*\.rpm')

cp "$RPM" "$TMP_DIR/trident.rpm"

pushd "$TMP_DIR"
rpm2cpio "$RPM" | cpio -idmv
popd

mv "$TMP_DIR/usr/bin/trident" $2
