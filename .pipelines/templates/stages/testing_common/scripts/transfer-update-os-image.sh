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

# Define path to the COSI file
COSI_FILE="$ARTIFACTS_DIR/regular.cosi"
# Define path to the update COSI file, i.e. with randomized FS UUID required for A/B update testing
UPDATE_COSI_FILE="$ARTIFACTS_DIR/regular-update.cosi"
# Define path to the COSI file on the host
HOST_COSI_FILE="$DESTINATION_DIR/regular_v$VERSION.cosi"

# If needed, change the name/path of the COSI file based on verityRequired parameter
if [ "$VERITY_REQUIRED" = "true" ]; then
    echo "Transferring verity COSI file onto the host"
    COSI_FILE="$ARTIFACTS_DIR/verity.cosi"
    UPDATE_COSI_FILE="$ARTIFACTS_DIR/verity-update.cosi"
    HOST_COSI_FILE="$DESTINATION_DIR/verity_v$VERSION.cosi"

    # Before transferring the COSI file, randomize the FS UUID
    ./bin/mkcosi randomize-fs-uuid "$COSI_FILE" "$UPDATE_COSI_FILE" /boot
else
    echo "Transferring regular COSI file onto the host"
    # Create destination directory on the host
    ssh -o StrictHostKeyChecking=no -i "$SSH_KEY_PATH" "$USER_NAME"@"$HOST_IP" "sudo mkdir -p '$DESTINATION_DIR'"
    
    ./bin/mkcosi randomize-fs-uuid "$COSI_FILE" "$UPDATE_COSI_FILE" /
fi

# Prepare destination directory on the host
ssh -o StrictHostKeyChecking=no -i "$SSH_KEY_PATH" "$USER_NAME"@"$HOST_IP" "sudo chown '$USER_NAME:$USER_NAME' '$DESTINATION_DIR' && sudo chmod 755 '$DESTINATION_DIR'"

# SCP the file onto the host
scp -o StrictHostKeyChecking=no -i "$SSH_KEY_PATH" "$UPDATE_COSI_FILE" "$USER_NAME"@"$HOST_IP":"$HOST_COSI_FILE"
echo "Transferred COSI file to the host"
