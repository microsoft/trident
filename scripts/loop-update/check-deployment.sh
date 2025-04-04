#!/bin/bash
set -euxo pipefail

. $(dirname $0)/common.sh

VM_IP=`getIp`

# Help diagnose https://dev.azure.com/mariner-org/ECF/_workitems/edit/11273 and
# fail explicitly if multiple IPs are found
if [ "$TEST_PLATFORM" == "qemu" ]; then
    if [ `echo $VM_IP | wc -w` -gt 1 ]; then
        echo "Multiple IPs found:"
        echo $VM_IP
        sudo virsh domifaddr $VM_NAME 
        adoError "Multiple IPs found"
        exit 1
    fi
fi

checkActiveVolume "volume-a" 0