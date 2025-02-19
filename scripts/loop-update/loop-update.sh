#!/bin/bash
set -euo pipefail

. $(dirname $0)/common.sh

if [ "$OUTPUT" != "" ]; then
    mkdir -p $OUTPUT
fi

# When ROLLBACK is set to true, the script will trigger a rollback scenario
# during the first update iteration. The rollback scenario will ensure that post
# rebooting into the updated OS, the OS will reboot again, thus letting the
# firmware boot back into the original OS, as the update was never completed
# successfully, since Trident did not run post reboot to certify the update.
ROLLBACK=${ROLLBACK:-false}

killUpdateServer $UPDATE_PORT_A
killUpdateServer $UPDATE_PORT_B

ls -l $ARTIFACTS/update-a
ls -l $ARTIFACTS/update-b

COSI_FILES=$(find $ARTIFACTS/update-a -type f -name '*.cosi')
COSI_FILES_COUNT=$(echo $COSI_FILES | wc -l)

if [ $COSI_FILES_COUNT -lt 1 ]; then
    echo "COSI file not found!"
    exit 1
elif [ $COSI_FILES_COUNT -gt 1 ]; then
    echo "Multiple COSI files found:"
    echo $COSI_FILES
    exit 1
else
    echo "COSI file found: $COSI_FILES"
fi

COSI_FILE=$(echo $COSI_FILES | head -1)
COSI_FILE=$(basename $COSI_FILE)

$ARTIFACTS/bin/netlisten -p $UPDATE_PORT_A -s $ARTIFACTS/update-a --force-color --full-logstream logstream-full-update-a.log &
$ARTIFACTS/bin/netlisten -p $UPDATE_PORT_B -s $ARTIFACTS/update-b --force-color --full-logstream logstream-full-update-a.log &

EXPECTED_VOLUME=${EXPECTED_VOLUME:-volume-b}
UPDATE_CONFIG=/var/lib/trident/update-config.yaml
# When triggering A/B update to B, we want to use images in update-config.yaml; to A, we want to
# use update-config2.yaml. However, if this is the rollback scenario, EXPECTED_VOLUME is current
# volume, so the value of UPDATE_CONFIG needs to be flipped.
if [ "$EXPECTED_VOLUME" == "volume-a" ] && [ "$ROLLBACK" == "false" ]; then
    UPDATE_CONFIG=/var/lib/trident/update-config2.yaml
elif [ "$EXPECTED_VOLUME" == "volume-b" ] && [ "$ROLLBACK" == "true" ]; then
    UPDATE_CONFIG=/var/lib/trident/update-config2.yaml
fi

RETRY_COUNT=${RETRY_COUNT:-20}

VM_IP=`getIp`

# Update the update-config.yaml file with the COSI file and host address address
# of the http server
sshCommand "sudo sed -i 's!verity.cosi!files/$COSI_FILE!' /var/lib/trident/update-config.yaml"
sshCommand "sudo sed -i 's/192.168.122.1/localhost/' /var/lib/trident/update-config.yaml"

sshCommand "sudo cp /var/lib/trident/update-config.yaml /var/lib/trident/update-config2.yaml && sudo sed -i 's/8000/8001/' /var/lib/trident/update-config2.yaml"

for i in $(seq 1 $RETRY_COUNT); do

    if [ "$TEST_PLATFORM" == "qemu" ]; then
        # For every 10th update, reboot the VM to ensure that we can handle reboots
        if [ $((i % 10)) -eq 0 ]; then
            echo ""
            echo "***************************"
            echo "** Rebooting VM          **"
            echo "***************************"
            echo ""

            truncateLog
            sudo virsh shutdown $VM_NAME
            until [ `sudo virsh list | grep -c $VM_NAME` -eq 0 ]; do sleep 1; done
            sudo virsh start $VM_NAME
            waitForLogin $i
        fi
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

    sshProxyPort $UPDATE_PORT_A
    sshProxyPort $UPDATE_PORT_B

    if sshCommand "ls /var/crash/*"; then
        echo "Crash files found on host"
        adoError "Crash files found on host during iteration $i"
        exit 1
    fi

    # If this is a rollback scenario, inject the script to trigger rollback into UPDATE_CONFIG
    if [ "$ROLLBACK" == "true" ] && [ $i -eq 1 ]; then
        TRIGGER_ROLLBACK_SCRIPT=.pipelines/templates/stages/testing_common/scripts/trigger-rollback.sh
        SCRIPT_HOST_COPY=/var/lib/trident/trigger-rollback.sh
        sshCommand "sudo tee $SCRIPT_HOST_COPY > /dev/null" < $TRIGGER_ROLLBACK_SCRIPT
        sshCommand "sudo chmod +x $SCRIPT_HOST_COPY"

        # The VM host does not have yq installed, so create a local copy of the update config
        # and inject the trigger-rollback script into it
        COPY_CONFIG="./config.yaml"
        sshCommand "sudo cat $UPDATE_CONFIG" > $COPY_CONFIG
        yq eval ".scripts.postProvision += [{
            \"name\": \"mount-var\",
            \"runOn\": [\"ab-update\"],
            \"content\": \"mkdir -p \$TARGET_ROOT/tmp/var && mount --bind /var \$TARGET_ROOT/tmp/var\"
        }]" -i $COPY_CONFIG
        yq eval "
        .scripts.postConfigure += [{
            \"name\": \"trigger-rollback\",
            \"runOn\": [\"ab-update\"],
            \"path\": \"$SCRIPT_HOST_COPY\"
        }]
        " -i $COPY_CONFIG

        # Set writableEtcOverlayHooks flag under internalParams to true, so that the script
        # can create a new systemd service
        yq eval ".internalParams.writableEtcOverlayHooks = true" -i $COPY_CONFIG
        sshCommand "sudo tee $UPDATE_CONFIG > /dev/null" < $COPY_CONFIG

        # Print out the contents of the update config to validate that the script was injected
        echo "Updated Host Configuration:"
        sshCommand "sudo cat $UPDATE_CONFIG"
    fi

    sshCommand "sudo cat $UPDATE_CONFIG"

    # Masking errors as we want to report the specific failure if it happens
    set +e

    sshCommand "sudo trident run $LOGGING -c $UPDATE_CONFIG --allowed-operations stage"
    STAGE_RESULT=$?

    if [ "$OUTPUT" != "" ]; then
        scp -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null $SSH_USER@$VM_IP:/var/log/trident-full.log $OUTPUT/staged-trident-full-$i.log
    fi

    if [ $STAGE_RESULT -ne 0 ]; then
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

    if [ "$TEST_PLATFORM" == "qemu" ]; then
        waitForLogin $i
    elif [ "$TEST_PLATFORM" == "azure" ]; then
        sleep 15
        SUCCESS=false
        for j in $(seq 1 10); do
            if sshCommand hostname; then
                SUCCESS=true
                break
            fi
            sleep 5
        done
        if [ "$SUCCESS" == false ]; then
            echo "VM did not come back up after update"
            adoError "VM did not come back up after update for iteration $i"
            exit 1
        fi
    fi
    set -e

    # Check that Trident updated correctly
    NEW_IP=`getIp`
    if [ "$NEW_IP" != "$VM_IP" ]; then
        echo "VM IP changed from $VM_IP to $NEW_IP"
        exit 1
    fi
    checkActiveVolume $EXPECTED_VOLUME $i

    # If this is a rollback scenario and we're on 1st iteration, validate that firmware
    # performed a rollback and that Trident detected it successfully
    if [ "$ROLLBACK" == "true" ] && [ $i -eq 1 ]; then
        validateRollback
    fi

    if [ $VERBOSE == True ]; then
        sshCommand "sudo trident get"
    fi

    if [ "$EXPECTED_VOLUME" == "volume-a" ]; then
        EXPECTED_VOLUME="volume-b"
        # If this is a rollback scenario and we're on 1st iteration, do not update the config path
        if [ "$ROLLBACK" != "true" ] || [ $i -ne 1 ]; then
            UPDATE_CONFIG="/var/lib/trident/update-config.yaml"
        fi
    else
        EXPECTED_VOLUME="volume-a"
        if [ "$ROLLBACK" != "true" ] || [ $i -ne 1 ]; then
            UPDATE_CONFIG="/var/lib/trident/update-config2.yaml"
        fi
    fi
done