#!/bin/bash
# Download base image from Azure storage.
# Expected env vars: BASE_IMG_TYPE, ARTIFACT_STAGING_DIR
set -eux

case ${BASE_IMG_TYPE} in
  ubuntu_2204_amd64)
    BLOB_NAME="ubuntu/server-cloudimg-2204-amd64/20260309/image.vhdx"
    ;;
  ubuntu_2204_arm64)
    BLOB_NAME="ubuntu/server-cloudimg-2204-arm64/20260309/image.vhdx"
    ;;
  ubuntu_2404_amd64)
    BLOB_NAME="ubuntu/server-cloudimg-2404-amd64/20260309/image.vhdx"
    ;;
  ubuntu_2404_arm64)
    BLOB_NAME="ubuntu/server-cloudimg-2404-arm64/20260309/image.vhdx"
    ;;
  gb200_2404_arm64)
    BLOB_NAME="gb200/arm64/20260318/image.vhdx"
    ;;
esac

mkdir -p "${ARTIFACT_STAGING_DIR}/images"
az storage blob download \
  --max-connections 10 \
  --auth-mode login \
  --account-name azlinuxbmpstaging \
  --container-name os-image-cache \
  --name "$BLOB_NAME" \
  --file "${ARTIFACT_STAGING_DIR}/images/${BASE_IMG_TYPE}.vhdx"
