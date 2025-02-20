#!/bin/bash
set -euo pipefail

. $(dirname "$0")/common.sh

if [ "$TEST_PLATFORM" == "qemu" ]; then
    virsh destroy "$VM_NAME" || true
    virsh undefine "$VM_NAME" --nvram || true
    cp "$ARTIFACTS/trident-vm-verity-testimage.qcow2" "$ARTIFACTS/booted.qcow2"

    sudo virt-install \
        --name "$VM_NAME" \
        --memory 2048 \
        --vcpus 2 \
        --os-variant generic \
        --import \
        --disk "$ARTIFACTS/booted.qcow2,bus=sata" \
        --network default \
        --boot uefi,loader=/usr/share/OVMF/OVMF_CODE_4M.fd,loader_secure=no \
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
        ensureAzureAccess
    fi
    if [ "`az group exists -n "$TEST_RESOURCE_GROUP"`" == "true" ]; then
        az group delete -n "$TEST_RESOURCE_GROUP" -y
    fi

    az group create -n "$TEST_RESOURCE_GROUP" -l "$PUBLISH_LOCATION" --tags creationTime=$(date +%s)

    VERSION=`getImageVersion`
    az vm create \
        --resource-group "$TEST_RESOURCE_GROUP" \
        --name "$VM_NAME" \
        --size "$TEST_VM_SIZE" \
        --os-disk-size-gb 60 \
        --admin-username "$SSH_USER" \
        --ssh-key-values "$SSH_PUBLIC_KEY_PATH" \
        --image "/subscriptions/$SUBSCRIPTION/resourceGroups/$GALLERY_RESOURCE_GROUP/providers/Microsoft.Compute/galleries/$GALLERY_NAME/images/$IMAGE_DEFINITION/versions/$VERSION" \
        --location "$PUBLISH_LOCATION"
    az vm boot-diagnostics enable --name "$VM_NAME" -g "$TEST_RESOURCE_GROUP"

    VM_IP=`az vm show -d -g "$TEST_RESOURCE_GROUP" -n "$VM_NAME" --query publicIps -o tsv`

    # Use az cli to confirm the VM deployment status is successful
    while [ "`az vm show -d -g "$TEST_RESOURCE_GROUP" -n "$VM_NAME" --query provisioningState -o tsv`" != "Succeeded" ]; do sleep 1; done
fi
