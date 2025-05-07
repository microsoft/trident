#!/bin/bash
# transfer-update-os-image.sh

set -eux

# Arguments
SSH_KEY_PATH=$1
USER_NAME=$2
HOST_IP=$3
ARTIFACTS_DIR=$4
DESTINATION_DIR=$5
VERSION=$6
VERITY_REQUIRED=$7

FILE_NAME_BASE="regular"

if [ "$VERITY_REQUIRED" = "true" ]; then
    FILE_NAME_BASE="verity"
fi

COSI_FILE_NAME="${FILE_NAME_BASE}_v${VERSION}.cosi"
LOCAL_FILE="$ARTIFACTS_DIR/$COSI_FILE_NAME"
REMOTE_FILE="$DESTINATION_DIR/$COSI_FILE_NAME"

echo "Transferring $LOCAL_FILE to the host into $REMOTE_FILE"

# Create destination directory on the host it it doesn't exist
ssh -o StrictHostKeyChecking=no -i "$SSH_KEY_PATH" "$USER_NAME"@"$HOST_IP" "sudo mkdir -p '$DESTINATION_DIR'"

# Prepare destination directory on the host
ssh -o StrictHostKeyChecking=no -i "$SSH_KEY_PATH" "$USER_NAME"@"$HOST_IP" "sudo chown '$USER_NAME:$USER_NAME' '$DESTINATION_DIR' && sudo chmod 755 '$DESTINATION_DIR'"

# SCP the file onto the host
scp -o StrictHostKeyChecking=no -i "$SSH_KEY_PATH" "$LOCAL_FILE" "$USER_NAME"@"$HOST_IP":"$REMOTE_FILE"
echo "Transferred COSI file to the host"
