#!/bin/bash
set -ex
trap '/bin/bash' ERR

# Mount CD-ROM using symlink
mkdir -p /mnt/cdrom
mount /dev/cdrom /mnt/cdrom

LIVEINSTALLER_PATH="/mnt/cdrom/installer/liveinstaller"
POSSIBLE_HC_TEMPLATE="/mnt/cdrom/installer/trident-config.yaml.tmpl"
IMAGES_DIR="/mnt/cdrom/images/"
TRIDENT_CONFIG="/etc/trident/config.yaml"
WORKING_DIR="/root/installer"

# Copy to execute liveinstaller
cp "$LIVEINSTALLER_PATH" "$WORKING_DIR"
cp "$POSSIBLE_HC_TEMPLATE" "$WORKING_DIR/template.yaml"

# Check if placeholder magic string is present (not patched)
if grep -q "^#8505c8ab802dd717290331acd0592804c4e413b030150c53f5018ac998b7831d:" "$WORKING_DIR/template.yaml"; then
    # Placeholder found - execute attended installation (interactive UI)
    cd "$WORKING_DIR"
    "$WORKING_DIR/liveinstaller" \
      --images-dir=$IMAGES_DIR \
      --host-config-output=$TRIDENT_CONFIG \
      --log-level=trace \
      --log-file=$WORKING_DIR/liveinstaller.log 2>&1 | tee "$WORKING_DIR/output_liveinstaller.log"
else
    # Valid config found - execute unattended installation
    echo "Pre-configured Host Configuration found. Starting unattended installation..."
    cd "$WORKING_DIR"
    "$WORKING_DIR/liveinstaller" \
      --unattended \
      --template-file=$WORKING_DIR/template.yaml \
      --log-level=trace \
      --log-file=$WORKING_DIR/liveinstaller.log 2>&1 | tee "$WORKING_DIR/output_liveinstaller.log"
fi

/bin/bash