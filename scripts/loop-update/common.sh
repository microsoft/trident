#!/bin/bash
set -euxo pipefail

ARTIFACTS=${ARTIFACTS:-artifacts}
VM_NAME=${VM_NAME:-trident-vm-verity-test}
VM_SERIAL_LOG=${VM_SERIAL_LOG:-/tmp/$VM_NAME.log}
VERBOSE=${VERBOSE:-False}
OUTPUT=${OUTPUT:-}

# Third parent of this script
TRIDENT_SOURCE_DIRECTORY=$(dirname $(dirname $(dirname $(realpath $0))))
SSH_USER=testuser

function getIp() {
    while [ `sudo virsh domifaddr $VM_NAME | grep -c "ipv4"` -eq 0 ]; do sleep 1; done
    sudo virsh domifaddr $VM_NAME | grep ipv4 | awk '{print $4}' | cut -d'/' -f1
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
    sudo truncate -s 0 "$VM_SERIAL_LOG"
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
