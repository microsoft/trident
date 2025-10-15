#!/bin/bash
set -ex
trap '/bin/bash' ERR

# Mount CD-ROM using symlink
mkdir -p /mnt/cdrom
mount /dev/cdrom /mnt/cdrom

INSTALLER_DIR="/mnt/cdrom/installer/"
IMAGES_DIR="/mnt/cdrom/images/"
TRIDENT_CONFIG="/etc/trident/config.yaml"
WORKING_DIR="/root/installer"
# IMAGES_DIR="$WORKING_DIR/images/"

# Copy the installer files to the working directory (merge with existing)
cp -r "$INSTALLER_DIR"* "$WORKING_DIR"

# Copy images from ISO to working directory
# cp -r "$IMAGES_ISO_DIR" "$WORKING_DIR"

cd "$WORKING_DIR"
"$WORKING_DIR/liveinstaller" \
  --images-dir=$IMAGES_DIR \
  --host-config-output=$TRIDENT_CONFIG \
  --log-level=trace \
  --log-file=$WORKING_DIR/liveinstaller.log 2>&1 | tee "$WORKING_DIR/output_liveinstaller.log"

/bin/bash