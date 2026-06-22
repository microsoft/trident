#!/bin/bash

# Takes a path to a directory containing a Trident RPM and extracts the Trident
# binary to the specified path.

set -eux

RPM_DIR=$1
OUTPUT_PATH=$2
ARCHITECTURE=$3
DISTRO=$4

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

case "$DISTRO" in
  azl3 | azl4)
    ;;
  *)
    echo "Unsupported distro: $DISTRO"
    exit 1
    ;;
esac

TMP_DIR=$(mktemp -d)
RPM=$(find "$RPM_DIR" | grep -P "trident-\d.*\.${DISTRO}\.${RPM_ARCH}\.rpm" | head -n 1 || true)
if [ -z "$RPM" ]; then
  echo "No Trident RPM found in '$RPM_DIR' matching distro=${DISTRO} arch=${RPM_ARCH}"
  exit 1
fi

cp "$RPM" "$TMP_DIR/trident.rpm"

pushd "$TMP_DIR"
rpm2cpio trident.rpm | cpio -idmv
popd

# OUTPUT_PATH is provided by the caller and determines distro layout
# (azl3 at the artifact base for backward compatibility, azl4 under azl4/).
mkdir -p "$OUTPUT_PATH"
mv "$TMP_DIR/usr/bin/trident" "$OUTPUT_PATH"

# Extract trident-acl-agent binary from the acl-agent sub-package RPM
ACL_AGENT_RPM=$(find "$RPM_DIR" | grep -P "trident-acl-agent-\d.*\.${DISTRO}\.${RPM_ARCH}\.rpm" | head -n 1 || true)
if [ -n "$ACL_AGENT_RPM" ]; then
  ACL_TMP_DIR=$(mktemp -d)
  cp "$ACL_AGENT_RPM" "$ACL_TMP_DIR/trident-acl-agent.rpm"

  pushd "$ACL_TMP_DIR"
  rpm2cpio trident-acl-agent.rpm | cpio -idmv
  popd

  mv "$ACL_TMP_DIR/usr/bin/trident-acl-agent" "$OUTPUT_PATH"
fi
