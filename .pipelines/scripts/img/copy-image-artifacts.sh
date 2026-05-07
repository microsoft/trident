#!/bin/bash
# Copy built image artifacts to the output directory.
# Expected env vars: OUTPUT_DIR, MAKE_TARGET
set -ex

mkdir -p "${OUTPUT_DIR}"

# images that have build/**/ format are expected to have multiple output files
if [ -d "build/${MAKE_TARGET}" ]; then
  sudo mv -v "build/${MAKE_TARGET}/"* "${OUTPUT_DIR}/"
else
  # Everything else is a file
  sudo mv -v "${MAKE_TARGET}" "${OUTPUT_DIR}/"
fi
