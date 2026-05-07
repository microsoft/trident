#!/bin/bash
# Run QEMU servicing test using storm-trident.
# Expected env vars: TRIDENT_SOURCE_DIR, STORM_FLAGS, PLATFORM,
#                    SSH_PRIVATE_KEY_PATH, SSH_PUBLIC_KEY_PATH,
#                    UPDATE_ITERATION_COUNT, BUILD_ID, OB_OUTPUT_DIR
set -eux

sudo ./bin/storm-trident run servicing ${STORM_FLAGS} \
  --artifacts-dir "${ARTIFACTS}" \
  --output-path "${OB_OUTPUT_DIR}" \
  --platform "${PLATFORM}" \
  --ssh-private-key-path "${SSH_PRIVATE_KEY_PATH}" \
  --ssh-public-key-path "${SSH_PUBLIC_KEY_PATH}" \
  --retry-count "${UPDATE_ITERATION_COUNT}" \
  --build-id "${BUILD_ID}" \
  --use-private-ip-address \
  --force-cleanup
set +x
echo "##vso[task.setvariable variable=STORM_SCENARIO_FINISHED;]true"
