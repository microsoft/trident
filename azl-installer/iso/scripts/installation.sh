#!/bin/bash
set -ex
trap '/bin/bash' ERR

CD_INSTALLER_DIR="/mnt/trident_cdrom/installer"
WORKING_DIR="/root/installer"
TRIDENT_CONFIG="/etc/trident/config.yaml"
TRIDENT_SCRIPTS="/etc/trident/scripts"
TRIDENT_PASSWORD_SCRIPT="$TRIDENT_SCRIPTS/user-password.sh"
TRIDENT_IMAGE_PATH="/mnt/trident_cdrom/images/azure-linux-trident.cosi"

cp -r "$CD_INSTALLER_DIR/" "/root/"

cd "$WORKING_DIR"
"$WORKING_DIR/liveinstaller" \
  --build-dir=$WORKING_DIR/ \
  --image-path=$TRIDENT_IMAGE_PATH \
  --attended \
  --log-level=trace \
  --log-file=$WORKING_DIR/liveinstaller.log > "$WORKING_DIR/output_liveinstaller.log" 2>&1

/bin/trident install
/bin/bash