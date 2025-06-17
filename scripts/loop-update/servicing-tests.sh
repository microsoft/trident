#!/bin/bash
set -euxo pipefail

ARTIFACTS=${ARTIFACTS:-artifacts}
VM_NAME=${VM_NAME:-trident-vm-verity-test}
TEST_PLATFORM=${TEST_PLATFORM:-qemu}
VM_SERIAL_LOG=${VM_SERIAL_LOG:-/tmp/$VM_NAME.log}
VERBOSE=${VERBOSE:-False}
WATCH=${WATCH:-False}
OUTPUT=${OUTPUT:-}

ALIAS=${ALIAS:-`whoami`}

SUBSCRIPTION=${SUBSCRIPTION:-b8a0db63-c5fa-4198-8e2a-f9d6ff52465e} # CoreOS_AzureLinux_BMP_dev
IMAGE_DEFINITION=${IMAGE_DEFINITION:-trident-vm-grub-verity-azure-testimage}
RESOURCE_GROUP=${RESOURCE_GROUP:-azlinux_bmp_dev}
PUBLISH_LOCATION=${PUBLISH_LOCATION:-eastus2}
GALLERY_RESOURCE_GROUP=${GALLERY_RESOURCE_GROUP:-$ALIAS-trident-rg}
STORAGE_ACCOUNT=${STORAGE_ACCOUNT:-azlinuxbmpdev}
GALLERY_NAME=${GALLERY_NAME:-${ALIAS}_trident_gallery}
PUBLISHER=${PUBLISHER:-$ALIAS}
OFFER=${OFFER:-trident-vm-grub-verity-azure-offer}
export AZCOPY_AUTO_LOGIN_TYPE=${AZCOPY_AUTO_LOGIN_TYPE:-AZCLI}
TEST_RESOURCE_GROUP=${TEST_RESOURCE_GROUP:-$GALLERY_RESOURCE_GROUP-test}
TEST_VM_SIZE=${TEST_VM_SIZE:-Standard_D2ds_v5}
SSH_PRIVATE_KEY_PATH=${SSH_PRIVATE_KEY_PATH:-~/.ssh/id_rsa}
SSH_PUBLIC_KEY_PATH=${SSH_PUBLIC_KEY_PATH:-$SSH_PRIVATE_KEY_PATH.pub}
RETRY_COUNT=${RETRY_COUNT:-20}

SSH_USER=testuser
UPDATE_PORT_A=8000
UPDATE_PORT_B=8001


SUDO="sudo"
if [ "$TEST_PLATFORM" == "azure" ]; then
    az login --identity
    SUDO=""
fi

FLAGS=""
if [ "$VERBOSE" == "true" ]; then
    FLAGS="$FLAGS --verbose"
fi
if [ "$WATCH" == "true" ]; then
    FLAGS="$FLAGS -w"
fi

$SUDO ./bin/storm-trident run servicing $FLAGS \
    --artifacts-dir $ARTIFACTS \
    --output-path "$OUTPUT" \
    --storage-account-resource-group $RESOURCE_GROUP \
    --name $VM_NAME \
    --serial-log $VM_SERIAL_LOG \
    --platform $TEST_PLATFORM \
    --retry-count $RETRY_COUNT \
    --rollback-retry-count $RETRY_COUNT \
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
    --ssh-private-key-path $SSH_PRIVATE_KEY_PATH \
    --ssh-public-key-path $SSH_PUBLIC_KEY_PATH \
    --user $SSH_USER \
    --update-port-a $UPDATE_PORT_A \
    --update-port-b $UPDATE_PORT_B \
    --force-cleanup
