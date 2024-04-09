#!/bin/bash

# Takes a path to a directory containing a trident RPM and extracts the trident
# binary to the specified path.

set -eu

TMP_DIR=$(mktemp -d)
RPM=$(find $1 | grep -P 'trident-\d.*\.rpm')

cp "$RPM" "$TMP_DIR/trident.rpm"

pushd "$TMP_DIR"
7z e -y trident.rpm
zstd -f -d trident-*.cpio.zstd
7z e -y trident-*.cpio ./usr/bin/trident
popd

mv "$TMP_DIR/trident" $2
