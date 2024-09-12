#!/bin/bash
# check-ssh-connection.sh

set -eux

# Arguments
ssh_key_path=$1
user_name=$2
host_ip=$3

ls -l "$ssh_key_path"
sshSucceeded=false
waitTime=1  # Initial wait time set to 1 second

# Check SSH connection and exponentially increase wait time if it fails,
# so that the total wait time is 5+ minutes
for i in {1..10}; do
    if timeout 15 ssh -q -o "ConnectTimeout=10" -o "StrictHostKeyChecking=no" -i "$ssh_key_path" "${user_name}@${host_ip}" exit; then
        echo "SSH connection successful. Host is up and running."
        sshSucceeded=true
        break
    else
        echo "SSH connection failed. Waiting for $waitTime seconds before retrying..."
        sleep $waitTime
        # Double the wait time for the next iteration
        waitTime=$((waitTime * 2))
    fi
done

# If SSH connection did not succeed, fail
if [ "$sshSucceeded" != "true" ]; then
    echo "SSH connection with the host could not be established."
    exit 1
fi
