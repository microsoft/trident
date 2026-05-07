#!/bin/bash
# Move downloaded artifacts into the expected directory structure.
# Expected env vars: TRIDENT_SOURCE_DIR, ARTIFACT_STAGING_DIR
set -ex

base_dir="${TRIDENT_SOURCE_DIR}/artifacts"

mkdir -p "$base_dir"
# Check if there are any .vhdx files in the images directory and move them
if ls "${ARTIFACT_STAGING_DIR}/images/" | grep -q ".*\.vhdx$"; then
  mv "${ARTIFACT_STAGING_DIR}/images/"*.vhdx "$base_dir/"
  rm -rf "${ARTIFACT_STAGING_DIR}/images"
else
  echo "No base image found"
  exit 1
fi

if ls "${ARTIFACT_STAGING_DIR}/rpms/" | grep -q "rpms.tar.gz"; then
  mkdir -p "$base_dir/rpm-overrides"
  tar -xvf "${ARTIFACT_STAGING_DIR}/rpms/rpms.tar.gz" \
    --strip-components=2 \
    -C "$base_dir/rpm-overrides"
  rm "${ARTIFACT_STAGING_DIR}/rpms/rpms.tar.gz"
fi

find "${ARTIFACT_STAGING_DIR}"
if [ -d "${ARTIFACT_STAGING_DIR}/trident" ]; then
  rpm_dir="${TRIDENT_SOURCE_DIR}/bin/RPMS"
  mkdir -p "$rpm_dir"
  cp -r "${ARTIFACT_STAGING_DIR}/trident/"* "$rpm_dir/" || echo 'no rpms to copy'
  rm -rf "${ARTIFACT_STAGING_DIR}/trident"
fi
