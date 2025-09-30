#!/bin/bash
set -ex
trap '/bin/bash' ERR

# CD_INSTALLER_DIR="/mnt/trident_cdrom/installer"
# IMAGES_PATH="/mnt/trident_cdrom/images/"
WORKING_DIR="/root/installer"

# Copy the installer files to the working directory
# cp -r "$CD_INSTALLER_DIR/" "/root/"
# cp -r "$IMAGES_PATH" "$WORKING_DIR/"

TRIDENT_CONFIG="/etc/trident/config.yaml"
TRIDENT_IMAGE_PATH="$WORKING_DIR/images/azure-linux-trident.cosi"

cd "$WORKING_DIR"
"$WORKING_DIR/liveinstaller" \
  --image-path=$TRIDENT_IMAGE_PATH \
  --host-config=$TRIDENT_CONFIG \
  --log-level=trace \
  --log-file=$WORKING_DIR/liveinstaller.log > "$WORKING_DIR/output_liveinstaller.log" 2>&1

/bin/trident install
/bin/bash