#!/bin/bash
# Run QEMU servicing test using storm-trident.
# Expected env vars: TRIDENT_SOURCE_DIR, STORM_FLAGS, PLATFORM,
#                    UPDATE_ITERATION_COUNT, BUILD_ID, OB_OUTPUT_DIR
set -eux

sudo ./bin/storm-trident run servicing ${STORM_FLAGS} \
  --artifacts-dir "${ARTIFACTS}" \
  --output-path "${OB_OUTPUT_DIR}" \
  --platform "${PLATFORM}" \
  --ssh-private-key-path "$HOME/.ssh/id_rsa" \
  --ssh-public-key-path "$HOME/.ssh/id_rsa.pub" \
  --retry-count "${UPDATE_ITERATION_COUNT}" \
  --build-id "${BUILD_ID}" \
  --use-private-ip-address \
  --force-cleanup
set +x
echo "##vso[task.setvariable variable=STORM_SCENARIO_FINISHED;]true"
