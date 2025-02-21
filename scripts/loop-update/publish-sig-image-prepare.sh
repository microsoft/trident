#!/bin/bash

set -euxo pipefail

SCRIPTS_DIR="$( dirname "$0" )"
. "$SCRIPTS_DIR/common.sh"

az account set --subscription "$SUBSCRIPTION"

if [ -z "${STORAGE_CONTAINER_NAME:-}" ]; then
    echo "STORAGE_CONTAINER_NAME is not set. Exiting..."
    exit 1
fi

if [ -z "${IMAGE_DEFINITION:-}" ]; then
    echo "IMAGE_DEFINITION is not set. Exiting..."
    exit 1
fi
# Ensure access when running in Azure DevOps, since this has not been
# working reliably in the past
if [ ! -z "${BUILD_BUILDNUMBER:-}" ]; then
    ensureAzureAccess "$RESOURCE_GROUP"
fi

if [ "`az group exists -n "$RESOURCE_GROUP"`" == "false" ]; then
    az group create -n "$RESOURCE_GROUP" -l "$PUBLISH_LOCATION"
fi
if [ "`az group exists -n "$GALLERY_RESOURCE_GROUP"`" == "false" ]; then
    az group create -n "$GALLERY_RESOURCE_GROUP" -l "$PUBLISH_LOCATION"
fi

# Ensure STORAGE_ACCOUNT exists and the managed identity has access
STORAGE_ACCOUNT_RESOURCE_ID="/subscriptions/$SUBSCRIPTION/resourceGroups/$RESOURCE_GROUP/providers/Microsoft.Storage/storageAccounts/$STORAGE_ACCOUNT"
if ! az storage account show --ids "$STORAGE_ACCOUNT_RESOURCE_ID"; then
    echo "Could not find storage account '$STORAGE_ACCOUNT' in the expected location. Creating the storage account."

    if [ "`az storage account check-name --name "$STORAGE_ACCOUNT" --query nameAvailable`" == "false" ]; then
        echo "Storage account name $STORAGE_ACCOUNT is not available"
        exit 1
    fi
    az storage account create -g "$RESOURCE_GROUP" -n "$STORAGE_ACCOUNT" -l "$PUBLISH_LOCATION" --allow-shared-key-access false
fi

# Ensure "build_target" storage container exists
CONTAINER_EXISTS="$(az storage container exists --account-name "$STORAGE_ACCOUNT" --name "$STORAGE_CONTAINER_NAME" --auth-mode login | jq .exists)"
if [[ "$CONTAINER_EXISTS" != "true" ]]; then
    echo "Could not find container '$STORAGE_CONTAINER_NAME'. Creating container '$STORAGE_CONTAINER_NAME' in storage account '$STORAGE_ACCOUNT'..."
    az storage container create --account-name "$STORAGE_ACCOUNT" --name "$STORAGE_CONTAINER_NAME" --auth-mode login
fi

# Ensure STEAMBOAT_GALLERY_NAME exists
if ! az sig show -r "$GALLERY_NAME" -g "$GALLERY_RESOURCE_GROUP"; then
    echo "Could not find image gallery '$GALLERY_NAME' in resource group '$GALLERY_RESOURCE_GROUP'. Creating the gallery."
    az sig create -g "$GALLERY_RESOURCE_GROUP" -r "$GALLERY_NAME" -l "$PUBLISH_LOCATION"
fi

# Ensure the "build_target" image-definition exists
# Note: We publish only the VHD from the secure-prod the SIG
IMAGE_DEFINITION_EXISTS="$(az sig image-definition list -r "$GALLERY_NAME" -g "$GALLERY_RESOURCE_GROUP" | grep "name" | grep -c "$IMAGE_DEFINITION" || :;)" # the "|| :;" prevents grep from halting the script when it finds no matches and exits with exit code 1
if [[ "$IMAGE_DEFINITION_EXISTS" -eq 0 ]]; then
    echo "Could not find image-definition '$IMAGE_DEFINITION'. Creating definition '$IMAGE_DEFINITION' in gallery '$GALLERY_NAME'..."
    az sig image-definition create -i "$IMAGE_DEFINITION" --publisher "$PUBLISHER" --offer "$OFFER" --sku "$IMAGE_DEFINITION" -r "$GALLERY_NAME" -g "$GALLERY_RESOURCE_GROUP" --os-type Linux
fi

if ! which azcopy; then
    # Install az-copy dependency
    PIPELINE_AGENT_OS="$(cat "/etc/os-release" | grep "^ID=" | cut -d = -f 2)"
    PIPELINE_AGENT_OS_VERSION="$(cat "/etc/os-release" | grep "^VERSION_ID=" | cut -d = -f 2 | tr -d '"')"
    AZCOPY_DOWNLOAD_URL="https://packages.microsoft.com/config/$PIPELINE_AGENT_OS/$PIPELINE_AGENT_OS_VERSION/packages-microsoft-prod.deb"
    curl -sSL -O "$AZCOPY_DOWNLOAD_URL"
    CURL_STATUS=$?
    if [ $CURL_STATUS -ne 0 ]; then
    echo "Failed to download the debian package repo while attempting to install azcopy. The URL '$AZCOPY_DOWNLOAD_URL' returned the curl exit status: $CURL_STATUS"
    echo "Suggestion: Are you using a new, non-ubuntu, pipeline agent? If yes, add azcopy installation logic for the new build agent."
    exit 1
    fi
    sudo dpkg -i packages-microsoft-prod.deb
    rm packages-microsoft-prod.deb
    sudo apt-get update -y
    sudo apt-get install azcopy -y
    azcopy --version
    AZCOPY_STATUS=$?
    if [ $AZCOPY_STATUS -ne 0 ]; then
        echo "Failed to install azcopy."
        exit 1
    fi
fi
