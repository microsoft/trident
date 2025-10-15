#!/bin/bash
set -ex
trap '/bin/bash' ERR

# Mount CD-ROM using symlink
mkdir -p /mnt/cdrom
mount /dev/cdrom /mnt/cdrom

LIVEINSTALLER_PATH="/mnt/cdrom/installer/liveinstaller"
IMAGES_DIR="/mnt/cdrom/images/"
TRIDENT_CONFIG="/etc/trident/config.yaml"
WORKING_DIR="/root/installer"

# Copy to execute liveinstaller
cp "$LIVEINSTALLER_PATH" "$WORKING_DIR"

cd "$WORKING_DIR"
"$WORKING_DIR/liveinstaller" \
  --images-dir=$IMAGES_DIR \
  --host-config-output=$TRIDENT_CONFIG \
  --log-level=trace \
  --log-file=$WORKING_DIR/liveinstaller.log 2>&1 | tee "$WORKING_DIR/output_liveinstaller.log"

/bin/bash