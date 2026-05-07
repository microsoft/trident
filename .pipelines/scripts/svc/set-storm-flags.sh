#!/bin/bash
# Compute storm-trident flags based on test configuration.
# Expected env vars: VERBOSE_LOGGING, FLAVOR, TEST_SECURE_BOOT,
#                    ROLLBACK_TESTING, UPDATE_ITERATION_COUNT
set -eux

FLAGS="-a"
if [ "${VERBOSE_LOGGING}" == "True" ]; then
  FLAGS="$FLAGS --verbose"
fi
if [ "${FLAVOR}" != "uki" ]; then
  if [[ "${TEST_SECURE_BOOT}" == 'True' ]]; then
    FLAGS="$FLAGS --secure-boot"
  fi
fi
if [ "${ROLLBACK_TESTING}" == "True" ]; then
  FLAGS="$FLAGS --rollback-retry-count ${UPDATE_ITERATION_COUNT} --rollback"
fi
set +x
echo "##vso[task.setvariable variable=STORM_FLAGS;]$FLAGS"
