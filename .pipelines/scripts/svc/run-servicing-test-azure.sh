#!/bin/bash
# Run azure servicing test using storm-trident.
# Expected env vars: TRIDENT_SOURCE_DIR, STORM_FLAGS, ARTIFACTS,
#                    PLATFORM, SUBSCRIPTION, IMAGE_DEFINITION,
#                    STORAGE_ACCOUNT, RESOURCE_GROUP, TEST_RESOURCE_GROUP,
#                    SUBNET_ID, SSH_PRIVATE_KEY_PATH, SSH_PUBLIC_KEY_PATH,
#                    UPDATE_ITERATION_COUNT, BUILD_ID, OB_OUTPUT_DIR
set -eux

cd "${TRIDENT_SOURCE_DIR}"
./bin/storm-trident run servicing ${STORM_FLAGS} \
  --artifacts-dir "${ARTIFACTS}" \
  --output-path "${OB_OUTPUT_DIR}" \
  --subscription "${SUBSCRIPTION}" \
  --image-definition "${IMAGE_DEFINITION}" \
  --storage-account "${STORAGE_ACCOUNT}" \
  --storage-account-resource-group "${RESOURCE_GROUP}" \
  --test-resource-group "${TEST_RESOURCE_GROUP}" \
  --platform "${PLATFORM}" \
  --subnet-id "${SUBNET_ID}" \
  --ssh-private-key-path "${SSH_PRIVATE_KEY_PATH}" \
  --ssh-public-key-path "${SSH_PUBLIC_KEY_PATH}" \
  --retry-count "${UPDATE_ITERATION_COUNT}" \
  --update-port-a 8123 --update-port-b 8124 \
  --build-id "${BUILD_ID}" \
  --use-private-ip-address \
  --force-cleanup
set +x
echo "##vso[task.setvariable variable=STORM_SCENARIO_FINISHED;]true"
