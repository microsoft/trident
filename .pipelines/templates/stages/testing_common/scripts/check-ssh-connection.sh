#!/bin/bash
# check-ssh-connection.sh

set -eux

# Arguments
ssh_key_path=$1
user_name=$2
host_ip=$3
runtime_env=$4

WAIT_TRIDENT_SERVICE_FINISHED=1
if [[ "$runtime_env" == "container" ]]; then
    WAIT_TRIDENT_SERVICE_FINISHED=0
fi

ls -l "$ssh_key_path"
sshSucceeded=false
waitTimeSeconds=5  # Retry wait time set to 5 seconds

SSH_CMD_ARGS="-o StrictHostKeyChecking=no -i $ssh_key_path ${user_name}@${host_ip}"

# Check SSH connection so that the total wait time is 10 minutes
start_time=$(date +%s)
end_time=$((start_time + 600))  # 10 minutes = 600 seconds

while [ $(date +%s) -lt $end_time ]; do
    if timeout 15 ssh -q -o "ConnectTimeout=10" $SSH_CMD_ARGS exit; then
        echo "SSH connection successful. Host is up and running."
        if [[ "$WAIT_TRIDENT_SERVICE_FINISHED" == "1" ]]; then
            echo "Check for trident.service completion."
            set +e
            trident_status=$(ssh -q $SSH_CMD_ARGS sudo systemctl status trident.service --no-pager)
            echo -e ".... check trident services:\n$trident_status\n"
            set -e
            if echo $trident_status | grep "Loaded: loaded" -A 5 | grep "Active:" | grep "dead"; then
                sshSucceeded=true
                break
            fi
        else
            echo "Do not wait for trident.service completion."
            sshSucceeded=true
            break
        fi
    fi

    echo "Waiting for $waitTimeSeconds seconds before retrying..."
    sleep $waitTimeSeconds
done

# If SSH connection did not succeed, fail
if [ "$sshSucceeded" != "true" ]; then
    echo "SSH connection with the host could not be established."
    exit 1
fi
