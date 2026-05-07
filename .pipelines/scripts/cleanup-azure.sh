#!/bin/bash
# Clean up Azure resources if the servicing test didn't complete.
# Expected env vars: TRIDENT_SOURCE_DIR, STORM_SCENARIO_FINISHED,
#                    VERBOSE_LOGGING, PLATFORM, SUBSCRIPTION,
#                    TEST_RESOURCE_GROUP, OB_OUTPUT_DIR
set -eux

cd "${TRIDENT_SOURCE_DIR}"
# If platform is azure AND the test failed to finish, run cleanup to
# ensure there are no azure resources left behind
if [ "${STORM_SCENARIO_FINISHED}" != "true" ]; then
  FLAGS="-a"
  if [ "${VERBOSE_LOGGING}" == "True" ]; then
    FLAGS="$FLAGS --verbose"
  fi

  ./bin/storm-trident run servicing -a $FLAGS \
    --output-path "${OB_OUTPUT_DIR}" \
    --subscription "${SUBSCRIPTION}" \
    --test-resource-group "${TEST_RESOURCE_GROUP}" \
    --platform "${PLATFORM}" \
    --test-case-to-run cleanup-vm
fi
