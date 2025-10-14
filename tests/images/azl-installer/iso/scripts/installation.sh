#!/bin/bash
set -ex
trap '/bin/bash' ERR


INSTALLER_DIR="/mnt/installer/"
TRIDENT_CONFIG="/etc/trident/config.yaml"

# Copy the installer files to the working directory
cp -r "$INSTALLER_DIR/" "/root/"

WORKING_DIR="/root/installer"
IMAGES_DIR="$WORKING_DIR/images/"

cd "$WORKING_DIR"
"$WORKING_DIR/liveinstaller" \
  --images-dir=$IMAGES_DIR \
  --host-config-output=$TRIDENT_CONFIG \
  --log-level=trace \
  --log-file=$WORKING_DIR/liveinstaller.log > "$WORKING_DIR/output_liveinstaller.log" 2>&1

/bin/bash