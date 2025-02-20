#!/bin/bash
set -euxo pipefail

ARTIFACTS=${ARTIFACTS:-artifacts}
VM_NAME=${VM_NAME:-trident-vm-verity-test}
VM_SERIAL_LOG=${VM_SERIAL_LOG:-/tmp/$VM_NAME.log}
VERBOSE=${VERBOSE:-False}
OUTPUT=${OUTPUT:-}

ALIAS=${ALIAS:-`whoami`}

SUBSCRIPTION=${SUBSCRIPTION:-b8a0db63-c5fa-4198-8e2a-f9d6ff52465e} # CoreOS_ECF_Kubernetes_dev
IMAGE_DEFINITION=${IMAGE_DEFINITION:-trident-vm-verity-azure-testimage}
RESOURCE_GROUP=${RESOURCE_GROUP:-azlinux_bmp_dev}
PUBLISH_LOCATION=${PUBLISH_LOCATION:-eastus2}
GALLERY_RESOURCE_GROUP=${GALLERY_RESOURCE_GROUP:-$ALIAS-trident-rg}
STORAGE_ACCOUNT=${STORAGE_ACCOUNT:-azlinuxbmpdev}
GALLERY_NAME=${GALLERY_NAME:-${ALIAS}_trident_gallery}
PUBLISHER=${PUBLISHER:-$ALIAS}
OFFER=${OFFER:-trident-vm-verity-azure-offer}
export AZCOPY_AUTO_LOGIN_TYPE=${AZCOPY_AUTO_LOGIN_TYPE:-AZCLI}
TEST_RESOURCE_GROUP=${TEST_RESOURCE_GROUP:-$GALLERY_RESOURCE_GROUP-test}
TEST_VM_SIZE=${TEST_VM_SIZE:-Standard_D2ads_v5}
SSH_PUBLIC_KEY_PATH=${SSH_PUBLIC_KEY_PATH:-~/.ssh/id_rsa.pub}

# Third parent of this script
TRIDENT_SOURCE_DIRECTORY=$(dirname $(dirname $(dirname $(realpath $0))))
SSH_USER=testuser

UPDATE_PORT_A=8000
UPDATE_PORT_B=8001

function getIp() {
    if [ "$TEST_PLATFORM" == "qemu" ]; then
        while [ `sudo virsh domifaddr $VM_NAME | grep -c "ipv4"` -eq 0 ]; do sleep 1; done
        sudo virsh domifaddr $VM_NAME | grep ipv4 | awk '{print $4}' | cut -d'/' -f1
    elif [ "$TEST_PLATFORM" == "azure" ]; then
        az vm show -d -g $TEST_RESOURCE_GROUP -n $VM_NAME --query publicIps -o tsv
    fi
}

function sshCommand() {
    local COMMAND=$1

    # BatchMode - running from a script, disable any interactive prompts
    # ConnectTimeout - how long to wait for the connection to be established
    # ServerAliveCountMax - how many keepalive packets can be missed before the connection is closed
    # ServerAliveInterval - how often to send keepalive packets
    # StrictHostKeyChecking - disable host key checking; TODO: remove this and
    # use the known_hosts file instead
    # UserKnownHostsFile - disable known hosts file to simplify local runs
    ssh \
        -o BatchMode=yes \
        -o ConnectTimeout=10 \
        -o ServerAliveCountMax=3 \
        -o ServerAliveInterval=5 \
        -o StrictHostKeyChecking=no \
        -o UserKnownHostsFile=/dev/null \
        $SSH_USER@$VM_IP \
        "$COMMAND"
}

function sshProxyPort() {
    local PORT=$1

    ssh \
        -R $PORT:localhost:$PORT -N \
        -o BatchMode=yes \
        -o ConnectTimeout=10 \
        -o ServerAliveCountMax=3 \
        -o ServerAliveInterval=5 \
        -o StrictHostKeyChecking=no \
        -o UserKnownHostsFile=/dev/null \
        $SSH_USER@$VM_IP &
}

function adoError() {
    local MESSAGE=$1

    set +x
    echo "##vso[task.logissue type=error]$MESSAGE"
    set -x
}

function checkActiveVolume() {
    local VOLUME=$1
    local ITERATION=$2

    ACTIVE=`sshCommand "set -o pipefail; sudo systemd-run --pipe --property=After=trident.service trident get" | grep abActiveVolume | tr -d ' ' | cut -d':' -f2`
    if [ "$ACTIVE" != $VOLUME ]; then
        sshCommand "sudo trident get"
        echo "Active volume is not $VOLUME, but $ACTIVE"
        adoError "Active volume is not $VOLUME, but $ACTIVE for iteration $ITERATION"
        exit 1
    fi
}

function validateRollback() {
    HOST_STATUS=`sshCommand "set -o pipefail; sudo systemd-run --pipe --property=After=trident.service trident get"`
    # Validate that lastError.category is set to "servicing"
    CATEGORY=$(echo "$HOST_STATUS" | yq eval '.lastError.category' -)
    if [ "$CATEGORY" != "servicing" ]; then
        sshCommand "sudo trident get"
        echo "Category of last error is not 'servicing', but '$CATEGORY'"
        adoError "Category of last error is not 'servicing', but '$CATEGORY'"
        exit 1
    fi

    # Validate that lastError.error contains the expected content
    ERROR=$(echo "$HOST_STATUS" | yq eval '.lastError.error' -)
    if [[ "$ERROR" != *"!ab-update-reboot-check"* ]]; then
        echo "Type of last error is not '!ab-update-reboot-check', but '$ERROR'"
        adoError "Type of last error is not '!ab-update-reboot-check', but '$ERROR'"
        exit 1
    fi

    # Validate that lastError.message matches the expected format
    MESSAGE=$(echo "$HOST_STATUS" | yq eval '.lastError.message' -)
    if ! echo "$MESSAGE" | grep -Eq '^A/B update failed as host booted from .+ instead of the expected device .+$'; then
        echo "Message of last error does not match the expected format: '$MESSAGE'"
        adoError "Message of last error does not match the expected format: '$MESSAGE'"
        exit 1
    fi

    echo "Rollback validation succeeded"
}

function truncateLog() {
    if sudo virsh dominfo $VM_NAME > /dev/null; then
        sudo truncate -s 0 "$VM_SERIAL_LOG"
    fi
}

function waitForLogin() {
    set +e
    local ITERATION=$1

    LOGGING=""
    if [ $VERBOSE == True ]; then
        echo "VM serial log:"
        LOGGING="-v"
    fi

    # Keeping errors masked, as we want to handle the failure explicitly
    sudo $TRIDENT_SOURCE_DIRECTORY/e2e_tests/helpers/wait_for_login.py \
        -d "$VM_SERIAL_LOG" \
        -o ./serial.log \
        -t 120 \
        $LOGGING

    WAIT_FOR_LOGIN_EXITCODE=$?

    if [ "$OUTPUT" != "" ]; then
        mkdir -p $OUTPUT
        OUTPUT_FILENAME=serial-$ITERATION.log
        if [ "${ROLLBACK:-}" == "true" ]; then
            OUTPUT_FILENAME=rollback-serial-$ITERATION.log
        fi
        sudo cp ./serial.log $OUTPUT/$OUTPUT_FILENAME
    fi

    if [ $WAIT_FOR_LOGIN_EXITCODE -ne 0 ]; then
        echo "Failed to reach login prompt for the VM"
        adoError "Failed to reach login prompt for the VM for iteration $ITERATION"

        df -h
        exit $WAIT_FOR_LOGIN_EXITCODE
    fi
    set -e
}

function getLatestVersion() {
  local G_RG_NAME=$1
  local G_NAME=$2
  local I_NAME=$3

  # TODO improve the sorting
  az sig image-version list -g $G_RG_NAME -r $G_NAME -i $I_NAME --query '[].name' -o tsv | sort -t "." -k1,1n -k2,2n -k3,3n | tail -1
}

function getImageVersion() {
    local OP=${1:-}
    if [ -z "${BUILD_BUILDNUMBER:-}" ]; then
        image_version=$(getLatestVersion $GALLERY_RESOURCE_GROUP $GALLERY_NAME $IMAGE_DEFINITION)
        if [ -z $image_version ]; then
            image_version=0.0.1
        else
            if [ "$OP" == "increment" ]; then
                # Increment the semver version
                image_version=$(echo $image_version | awk -F. '{print $1"."$2"."$3+1}')
            fi
        fi
    else
        image_version="0.0.$BUILD_BUILDID"
    fi

    echo $image_version
}

function resizeImage() {
    local IMAGE_PATH=$1
    # VHD images on Azure must have a virtual size aligned to 1MB. https://learn.microsoft.com/en-us/azure/virtual-machines/linux/create-upload-generic#resize-vhds
    raw_file="resize.raw"
    sudo qemu-img convert -f vpc -O raw $IMAGE_PATH $raw_file
    MB=$((1024*1024))
    size=$(qemu-img info -f raw --output json "$raw_file" | \
    gawk 'match($0, /"virtual-size": ([0-9]+),/, val) {print val[1]}')

    rounded_size=$(((($size+$MB-1)/$MB)*$MB))

    echo "Rounded Size = $rounded_size"

    sudo qemu-img resize $raw_file $rounded_size

    sudo qemu-img convert -f raw -o subformat=fixed,force_size -O vpc $raw_file $IMAGE_PATH
}

function killUpdateServer() {
    local UPDATE_PORT=$1

    set +e
    KILL_PID=$(lsof -ti tcp:${UPDATE_PORT})
    PROCESS_FOUND=$?
    set -e
    if [ $PROCESS_FOUND -eq 0 ]; then
        echo "Process already running on the trident update server port: '${UPDATE_PORT}'. Killing process '$KILL_PID'."
        kill -9 $KILL_PID > /dev/null 2>&1 || true
    fi
}

function ensureAzureAccess() {
    # Ensure the managed identity has access to the subscription
    # 1cd7f210-4327-4ef9-b33f-f64d342cc431 is trident-servicing-test managed
    # identity, assigned to the pool
    for i in {1..10}; do
        if az role assignment list --assignee 1cd7f210-4327-4ef9-b33f-f64d342cc431 --scope "/subscriptions/$SUBSCRIPTION" | grep -q "Contributor"; then
            break;
        fi
        echo "Managed identity does not have access to the subscription, retrying..."
        sleep 5
    done
    echo "MSAL token cache client ids: "
    set +e
    ls ~/.azure
    set -e
    
    echo "Signed in user id: "
    az ad signed-in-user show --query id -o tsv
}