#!/bin/bash
# Publish a base image to Azure Shared Image Gallery.
# Expected env vars: TRIDENT_SOURCE_DIR, PLATFORM, ARTIFACTS,
#                    SUBSCRIPTION, IMAGE_DEFINITION, STORAGE_ACCOUNT,
#                    RESOURCE_GROUP, BUILD_ID, OB_OUTPUT_DIR
set -eux

cd "${TRIDENT_SOURCE_DIR}"
./bin/storm-trident run servicing -a \
  --output-path "${OB_OUTPUT_DIR}" \
  --platform "${PLATFORM}" \
  --artifacts-dir "${ARTIFACTS}" \
  --use-private-ip-address \
  --build-id "${BUILD_ID}" \
  --subscription "${SUBSCRIPTION}" \
  --image-definition "${IMAGE_DEFINITION}" \
  --storage-account "${STORAGE_ACCOUNT}" \
  --storage-account-resource-group "${RESOURCE_GROUP}" \
  --test-case-to-run publish-sig-image
