#!/bin/bash

set -euxo pipefail

SCRIPTS_DIR="$( dirname "$0" )"
. "$SCRIPTS_DIR/common.sh"

az account set --subscription  "$SUBSCRIPTION"

CURRENT_DATE="$(date +'%y%m%d')"
CURRENT_TIME="$(date +'%H%M%S')"

STORAGE_ACCOUNT_URL="https://$STORAGE_ACCOUNT.blob.core.windows.net"
STORAGE_ACCOUNT_RESOURCE_ID="/subscriptions/$SUBSCRIPTION/resourceGroups/$RESOURCE_GROUP/providers/Microsoft.Storage/storageAccounts/$STORAGE_ACCOUNT"

export STORAGE_CONTAINER_NAME="${STORAGE_CONTAINER_NAME:-$ALIAS-test}"
"$SCRIPTS_DIR/publish-sig-image-prepare.sh"
export IMAGE_PATH="${IMAGE_PATH:-$ARTIFACTS/trident-vm-verity-azure-testimage.vhd}"

IMAGE_VERSION="`getImageVersion increment`"
echo using image version $IMAGE_VERSION

if az sig image-version show \
  --resource-group "$GALLERY_RESOURCE_GROUP" \
  --gallery-name "$GALLERY_NAME" \
  --gallery-image-definition "$IMAGE_DEFINITION" \
  --gallery-image-version "$IMAGE_VERSION"; then
    echo "Image version $IMAGE_VERSION already exists. Exiting..."
    exit 0
fi

STORAGE_BLOB_NAME="${CURRENT_DATE##+(0)}.${CURRENT_TIME##+(0)}-$IMAGE_VERSION.vhd"
STORAGE_BLOB_ENDPOINT="$STORAGE_ACCOUNT_URL/$STORAGE_CONTAINER_NAME/$STORAGE_BLOB_NAME"

# Get the path to the VHD file
resizeImage "$IMAGE_PATH"

# Upload the image artifact to Steamboat Storage Account
azcopy copy "$IMAGE_PATH" "$STORAGE_BLOB_ENDPOINT"

# Create Image Version from storage account blob
az sig image-version create \
  --resource-group "$GALLERY_RESOURCE_GROUP" \
  --gallery-name "$GALLERY_NAME" \
  --gallery-image-definition "$IMAGE_DEFINITION" \
  --gallery-image-version "$IMAGE_VERSION" \
  --target-regions "$PUBLISH_LOCATION" \
  --location "$PUBLISH_LOCATION" \
  --os-vhd-storage-account "$STORAGE_ACCOUNT_RESOURCE_ID" \
  --os-vhd-uri "$STORAGE_BLOB_ENDPOINT"
