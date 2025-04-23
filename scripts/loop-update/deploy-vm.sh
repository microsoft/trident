#!/bin/bash
set -euo pipefail

. $(dirname "$0")/common.sh

if [ "$TEST_PLATFORM" == "qemu" ]; then
    sudo virsh list --all
    sudo virsh destroy "$VM_NAME" || true
    sudo virsh undefine "$VM_NAME" --nvram || true

    IMAGE_FILES="$(find $ARTIFACTS -type f -name 'trident-vm-*-testimage.qcow2')"
    IMAGE_FILES_COUNT=$(echo "$IMAGE_FILES" | wc -l)

    if [ $IMAGE_FILES_COUNT -lt 1 ]; then
        echo "Image file not found!"
        exit 1
    elif [ $IMAGE_FILES_COUNT -gt 1 ]; then
        echo "Multiple image files found:"
        echo $IMAGE_FILES
        exit 1
    else
        echo "Image file found: $IMAGE_FILES"
    fi

    IMAGE_FILE=$(echo $IMAGE_FILES | head -1)

    BOOT_IMAGE="$ARTIFACTS/booted.qcow2"
    cp "$IMAGE_FILE" "$BOOT_IMAGE"

    BOOT_CONFIG="--machine q35 --boot uefi,loader_secure=yes"
    if [ "${SECURE_BOOT:-}" == "False" ]; then
        BOOT_CONFIG="--boot uefi,loader_secure=no"
    fi

    sudo virt-install \
        --name "$VM_NAME" \
        --memory 2048 \
        --vcpus 2 \
        --os-variant generic \
        --import \
        --disk "$BOOT_IMAGE,bus=sata" \
        --network default \
        $BOOT_CONFIG \
        --noautoconsole \
        --serial "file,path=$VM_SERIAL_LOG"

    until [ -f "$VM_SERIAL_LOG" ]
    do
        sleep 0.1
    done

    waitForLogin 0
elif [ "$TEST_PLATFORM" == "azure" ]; then
    az account set -s "$SUBSCRIPTION"

    # Ensure access when running in Azure DevOps, since this has not been
    # working reliably in the past
    if [ ! -z "${BUILD_BUILDNUMBER:-}" ]; then
        ensureAzureAccess "$TEST_RESOURCE_GROUP"
    fi
    if [ "`azCommand group exists -n "$TEST_RESOURCE_GROUP"`" == "true" ]; then
        azCommand group delete -n "$TEST_RESOURCE_GROUP" -y
    fi

    azCommand group create -n "$TEST_RESOURCE_GROUP" -l "$PUBLISH_LOCATION" --tags creationTime=$(date +%s)

    if [ -n "${VALIDATION_SUBNET_ID:-}" ]; then
        SUBNET_ARG="--subnet $VALIDATION_SUBNET_ID"
    fi

    VERSION=`getImageVersion`
    azCommand vm create \
        --resource-group "$TEST_RESOURCE_GROUP" \
        --name "$VM_NAME" \
        --size "$TEST_VM_SIZE" \
        --os-disk-size-gb 60 \
        --admin-username "$SSH_USER" \
        --ssh-key-values "$SSH_PUBLIC_KEY_PATH" \
        --image "/subscriptions/$SUBSCRIPTION/resourceGroups/$GALLERY_RESOURCE_GROUP/providers/Microsoft.Compute/galleries/$GALLERY_NAME/images/$IMAGE_DEFINITION/versions/$VERSION" \
        --location "$PUBLISH_LOCATION" \
        --security-type TrustedLaunch \
        --enable-secure-boot true \
        --enable-vtpm true \
        --no-wait \
        $SUBNET_ARG

    # Attempt to enable the boot diagnostics early on
    while ! azCommand vm boot-diagnostics enable --name "$VM_NAME" -g "$TEST_RESOURCE_GROUP"; do
        sleep 1
    done

    # Wait for the boot diagnostics to be available
    while azCommand vm boot-diagnostics get-boot-log --name "$VM_NAME" --resource-group "$TEST_RESOURCE_GROUP" | grep "<Error><Code>BlobNotFound</Code><Message>"; do
        sleep 5
    done

    # Use az cli to confirm the VM deployment status is successful
    while [ "`azCommand vm show -d -g "$TEST_RESOURCE_GROUP" -n "$VM_NAME" --query provisioningState -o tsv`" != "Succeeded" ]; do sleep 1; done
fi
