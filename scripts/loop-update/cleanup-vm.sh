#!/bin/bash
set -euo pipefail

. $(dirname $0)/common.sh

if [ "$TEST_PLATFORM" == "azure" ]; then
    az account set -s $SUBSCRIPTION
    if [ "`az group exists -n $TEST_RESOURCE_GROUP`" == "true" ]; then
        az group delete -n $TEST_RESOURCE_GROUP -y
    fi
elif [ "$TEST_PLATFORM" == "qemu" ]; then
    virsh destroy $VM_NAME || true
    virsh undefine $VM_NAME --nvram || true
fi
killUpdateServer $UPDATE_PORT_A
killUpdateServer $UPDATE_PORT_B
