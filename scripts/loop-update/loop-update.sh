#!/bin/bash
set -euo pipefail

. $(dirname $0)/common.sh

pushd $ARTIFACTS/update-a
killall python3 || true
python3 -m http.server 8000 &
cd ../update-b
python3 -m http.server 8001 &
popd

EXPECTED_VOLUME=volume-b
UPDATE_CONFIG=/var/lib/trident/update-config.yaml
RETRY_COUNT=${RETRY_COUNT:-20}

VM_IP=`getIp`

sshCommand "sudo cp $UPDATE_CONFIG /var/lib/trident/update-config2.yaml && sudo sed -i 's/8000/8001/' /var/lib/trident/update-config2.yaml"

for i in $(seq 1 $RETRY_COUNT); do

    # For every 10th update, reboot the VM to ensure that we can handle reboots
    if [ $((i % 10)) -eq 0 ]; then
        echo ""
        echo "***************************"
        echo "** Rebooting VM          **"
        echo "***************************"
        echo ""

        truncateLog
        #sudo virsh reboot $VM_NAME
        sudo virsh shutdown $VM_NAME
        until [ `sudo virsh list | grep -c $VM_NAME` -eq 0 ]; do sleep 1; done
        sudo virsh start $VM_NAME
        waitForLogin $i
    fi

    echo ""
    echo "***************************"
    echo "** Starting update $i    **"
    echo "***************************"
    echo ""

    truncateLog
    LOGGING="-v WARN"
    if [ $VERBOSE == True ]; then
        LOGGING="-v INFO"
    fi

    # Masking errors as we want to report the specific failure if it happens
    set +e

    sshCommand "sudo trident run $LOGGING -c $UPDATE_CONFIG --allowed-operations stage"
    if [ $? -ne 0 ]; then
        echo "Failed to stage update"
        adoError "Failed to stage update for iteration $i"
        exit 1
    fi

    set -e

    # Masking errors as the VM will be rebooting
    set +e

    sshCommand "sudo trident run $LOGGING -c $UPDATE_CONFIG --allowed-operations finalize"

    LOGGING=""
    if [ $VERBOSE == True ]; then
        echo "VM serial log:"
        LOGGING="-v"
    fi

    waitForLogin $i
    set -e

    # Check that Trident updated correctly
    NEW_IP=`getIp`
    if [ "$NEW_IP" != "$VM_IP" ]; then
        echo "VM IP changed from $VM_IP to $NEW_IP"
        exit 1
    fi
    checkActiveVolume $EXPECTED_VOLUME $i
    if [ $VERBOSE == True ]; then
        sshCommand "sudo trident get"
    fi
    if [ $EXPECTED_VOLUME == volume-a ]; then
        EXPECTED_VOLUME=volume-b
        UPDATE_CONFIG=/var/lib/trident/update-config.yaml
    else
        EXPECTED_VOLUME=volume-a
        UPDATE_CONFIG=/var/lib/trident/update-config2.yaml
    fi
done