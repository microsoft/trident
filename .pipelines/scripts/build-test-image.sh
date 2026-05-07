#!/bin/bash
# Build a test image using the Makefile.
# Expected env vars: MIC_BUILD_TYPE, MIC_ARCHITECTURE, PIPELINE_ARCHITECTURE, MAKE_TARGET
set -ex

# Meta
echo "Local directory: $(pwd)"

echo "Base files:"
find artifacts/ -type f | sort

if [[ "${MIC_BUILD_TYPE}" == "dev" ]]; then
  export MIC_CONTAINER_IMAGE="imagecustomizer:dev"
fi

# Allow cross-platform (i.e. amd64 pipeline creating arm64 images)
if [[ "${MIC_ARCHITECTURE}" != "${PIPELINE_ARCHITECTURE}" ]]; then
  # Export variable to tell Makefile and testimages to use `docker --platform`
  export MIC_ARCHITECTURE="linux/${MIC_ARCHITECTURE}"
fi

# Compress the full image & delete the uncompressed image
make "${MAKE_TARGET}"
