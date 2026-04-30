#!/bin/bash

# Takes a path to a directory containing a Trident RPM and extracts the Trident
# binary to the specified path.

set -eux

RPM_DIR=$1
OUTPUT_PATH=$2
ARCHITECTURE=$3

RPM_ARCH=""
case "$ARCHITECTURE" in
  amd64)
    RPM_ARCH="x86_64"
    ;;
  arm64)
    RPM_ARCH="aarch64"
    ;;
  *)
    echo "Unsupported architecture: $ARCHITECTURE"
    exit 1
    ;;
esac

TMP_DIR=$(mktemp -d)
RPM=$(find $RPM_DIR | grep -P "trident-\d.*\.${RPM_ARCH}\.rpm")

cp "$RPM" "$TMP_DIR/trident.rpm"

pushd "$TMP_DIR"
rpm2cpio trident.rpm | cpio -idmv
popd

mv "$TMP_DIR/usr/bin/trident" $OUTPUT_PATH
