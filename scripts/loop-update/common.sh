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

    ssh -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null $SSH_USER@$VM_IP \
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
        sudo cp ./serial.log $OUTPUT/serial-$ITERATION.log
    fi

    if [ $WAIT_FOR_LOGIN_EXITCODE -ne 0 ]; then
        echo "Failed to reach login prompt for the VM"
        adoError "Failed to reach login prompt for the VM for iteration $ITERATION"

        df -h
        exit $WAIT_FOR_LOGIN_EXITCODE
    fi
    set -e
}
