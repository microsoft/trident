#!/bin/bash
set -ex
trap '/bin/bash' ERR


INSTALLER_DIR="/mnt/cdrom/installer/"
TRIDENT_CONFIG="/etc/trident/config.yaml"

# Copy the installer files to the working directory (merge with existing)
cp -r "$INSTALLER_DIR"* "/root/installer/"

# Copy images from ISO to working directory
mkdir -p "/root/installer/images"
cp /mnt/cdrom/images/azure-linux-trident.cosi /root/installer/images/
cp /mnt/cdrom/images/azure-linux-full.cosi /root/installer/images/

WORKING_DIR="/root/installer"
IMAGES_DIR="$WORKING_DIR/images/"

cd "$WORKING_DIR"
"$WORKING_DIR/liveinstaller" \
  --images-dir=$IMAGES_DIR \
  --host-config-output=$TRIDENT_CONFIG \
  --log-level=trace \
  --log-file=$WORKING_DIR/liveinstaller.log 2>&1 | tee "$WORKING_DIR/output_liveinstaller.log"

/bin/bash