#!/bin/bash
set -ex
trap '/bin/bash' ERR

CDROM_MOUNT="/mnt/cdrom"
WORKING_DIR="/root/installer"
MAGIC_PLACEHOLDER="8505c8ab802dd717290331acd0592804c4e413b030150c53f5018ac998b7831d"

# Mount CD-ROM using symlink
mkdir -p "$CDROM_MOUNT"
mount /dev/cdrom "$CDROM_MOUNT"

# CD-ROM paths
LIVEINSTALLER_PATH="$CDROM_MOUNT/installer/liveinstaller"
POSSIBLE_HC_TEMPLATE="$CDROM_MOUNT/installer/trident-config.yaml.tmpl"
IMAGES_DIR="$CDROM_MOUNT/images/"

# WORKING_DIR paths
LIVEINSTALLER_BIN="$WORKING_DIR/liveinstaller"
TRIDENT_CONFIG_DESTINATION="/etc/trident/config.yaml"
LOG_FILE="$WORKING_DIR/liveinstaller.log"
OUTPUT_LOG="$WORKING_DIR/output_liveinstaller.log"

# Copy to WORKING_DIR to execute liveinstaller
cp "$LIVEINSTALLER_PATH" "$LIVEINSTALLER_BIN"

# Check if placeholder magic string is present (ISO not patched)
if grep -q "^#${MAGIC_PLACEHOLDER}:" "$POSSIBLE_HC_TEMPLATE"; then
    # Placeholder found - execute attended installation (interactive UI)
    cd "$WORKING_DIR"
    "$LIVEINSTALLER_BIN" \
      --images-dir="$IMAGES_DIR" \
      --host-config-output="$TRIDENT_CONFIG_DESTINATION" \
      --log-level=trace \
      --log-file="$LOG_FILE" 2>&1 | tee "$OUTPUT_LOG"
else
    # Placeholder was replaced by a Host Configuration - execute unattended installation
    echo "Pre-configured Host Configuration found. Starting unattended installation..."
    # Save template in WORKING_DIR in case of failure
    TEMPLATE_FILE="$WORKING_DIR/template.yaml"
    cp "$POSSIBLE_HC_TEMPLATE" "$TEMPLATE_FILE"
    # Execute liveinstaller (unattended)
    cd "$WORKING_DIR"
    "$LIVEINSTALLER_BIN" \
      --unattended \
      --template-file="$TEMPLATE_FILE" \
      --log-level=trace \
      --log-file="$LOG_FILE" 2>&1 | tee "$OUTPUT_LOG"
fi

/bin/bash