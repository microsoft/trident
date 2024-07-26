#!/bin/bash
# copy-runtime-os-images.sh

set -eux

# Arguments
SSH_KEY_PATH=$1
USER_NAME=$2
HOST_IP=$3
ARTIFACTS_DIR=$4
LOCAL_TEMP_DIR=$5
DESTINATION_DIR=$6
VERSION=$7
VERITY_REQUIRED=$8

# Create temporary local directory for staging the files
mkdir -p "$LOCAL_TEMP_DIR"

# Copy files based on verityRequired parameter
if [ "$VERITY_REQUIRED" = "true" ]; then
    echo "Copying verity OS images to the local temp directory"
    cp "$ARTIFACTS_DIR/verity_boot.rawzst" "$LOCAL_TEMP_DIR/verity_boot_v$VERSION.rawzst"
    cp "$ARTIFACTS_DIR/verity_esp.rawzst" "$LOCAL_TEMP_DIR/verity_esp_v$VERSION.rawzst"
    cp "$ARTIFACTS_DIR/verity_root.rawzst" "$LOCAL_TEMP_DIR/verity_root_v$VERSION.rawzst"
    cp "$ARTIFACTS_DIR/verity_roothash.rawzst" "$LOCAL_TEMP_DIR/verity_roothash_v$VERSION.rawzst"
    cp "$ARTIFACTS_DIR/verity_var.rawzst" "$LOCAL_TEMP_DIR/verity_var_v$VERSION.rawzst"

    echo "Randomizing boot partition's filesystem UUID"
    RAW_FILE="$LOCAL_TEMP_DIR/verity_boot.raw"
    zstd --rm -d "$LOCAL_TEMP_DIR/verity_boot_v$VERSION.rawzst" -o "$RAW_FILE"
    e2fsck -f -p "$RAW_FILE"
    tune2fs -U random "$RAW_FILE"
    zstd --rm -T0 "$RAW_FILE" -o "$LOCAL_TEMP_DIR/verity_boot_v$VERSION.rawzst"
else
    echo "Copying runtime OS images to the local temp directory"
    cp "$ARTIFACTS_DIR/esp.rawzst" "$LOCAL_TEMP_DIR/esp_v$VERSION.rawzst"
    cp "$ARTIFACTS_DIR/root.rawzst" "$LOCAL_TEMP_DIR/root_v$VERSION.rawzst"

    echo "Randomizing root partition's filesystem UUID"
    RAW_FILE="$LOCAL_TEMP_DIR/root.raw"
    zstd --rm -d "$LOCAL_TEMP_DIR/root_v$VERSION.rawzst" -o "$RAW_FILE"
    e2fsck -f -p "$RAW_FILE"
    tune2fs -U random "$RAW_FILE"
    zstd --rm -T0 "$RAW_FILE" -o "$LOCAL_TEMP_DIR/root_v$VERSION.rawzst"
fi

# When copying non-verity images, create destination directory on the host
if [ "$VERITY_REQUIRED" = "false" ]; then
    ssh -o StrictHostKeyChecking=no -i "$SSH_KEY_PATH" "$USER_NAME"@"$HOST_IP" "sudo mkdir -p $DESTINATION_DIR"
fi

# Prepare destination directory on the host
ssh -o StrictHostKeyChecking=no -i "$SSH_KEY_PATH" "$USER_NAME"@"$HOST_IP" "sudo chown $USER_NAME:$USER_NAME $DESTINATION_DIR && sudo chmod 755 $DESTINATION_DIR"

# SCP the entire directory to the host
echo "Transferring local temp directory to the host"
scp -o StrictHostKeyChecking=no -i "$SSH_KEY_PATH" -r "$LOCAL_TEMP_DIR"/* "$USER_NAME"@"$HOST_IP":"$DESTINATION_DIR/"

# Remove the local temp directory
rm -rf "$LOCAL_TEMP_DIR"
