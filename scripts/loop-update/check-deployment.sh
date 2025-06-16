#!/bin/bash
set -euxo pipefail

. $(dirname $0)/common.sh

SUDO="sudo"
if [ "$TEST_PLATFORM" == "azure" ]; then
    SUDO=""
fi

$SUDO ./bin/storm-trident helper servicing-tests \
    --output-path $OUTPUT \
    --artifacts-dir $ARTIFACTS \
    --retry-count $RETRY_COUNT \
    --expected-volume volume-b \
    --storage-account-resource-group $RESOURCE_GROUP \
    --ssh-private-key-path ~/.ssh/id_rsa \
    --user $SSU_USER \
    --platform $TEST_PLATFORM \
    --name $VM_NAME \
    --serial-log $VM_SERIAL_LOG \
    --who-am-i $ALIAS \
    --subscription $SUBSCRIPTION \
    --image-definition $IMAGE_DEFINITION \
    --region $PUBLISH_LOCATION \
    --gallery-resource-group $GALLERY_RESOURCE_GROUP \
    --storage-account $STORAGE_ACCOUNT \
    --gallery-name $GALLERY_NAME \
    --offer $OFFER \
    --test-resource-group $TEST_RESOURCE_GROUP \
    --size $TEST_VM_SIZE \
    --ssh-public-key-path $SSH_PUBLIC_KEY_PATH \
    --user $SSH_USER \
    --update-port-a $UPDATE_PORT_A \
    --update-port-b $UPDATE_PORT_B \
    --expected-volume volume-a \
    --test-case-to-run check-deployment
