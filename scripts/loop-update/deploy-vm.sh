#/bin/bash

set -euo pipefail

. $(dirname $0)/common.sh

virsh destroy $VM_NAME || true
virsh undefine $VM_NAME --nvram || true
cp $ARTIFACTS/trident-vm-verity-testimage.qcow2 $ARTIFACTS/booted.qcow2

sudo virt-install \
    --name $VM_NAME \
    --memory 2048 \
    --vcpus 2 \
    --os-variant generic \
    --import \
    --disk $ARTIFACTS/booted.qcow2,bus=sata \
    --network default \
    --boot uefi,loader=/usr/share/OVMF/OVMF_CODE_4M.fd,loader_secure=no \
    --noautoconsole \
    --serial "file,path=$VM_SERIAL_LOG"

until [ -f "$VM_SERIAL_LOG" ]
do
    sleep 0.1
done

LOGGING=""
if [ $VERBOSE == True ]; then
    echo "Found VM serial log file: $VM_SERIAL_LOG"
    echo "VM serial log:"
    LOGGING="-v"
fi

sudo $TRIDENT_SOURCE_DIRECTORY/e2e_tests/helpers/wait_for_login.py \
    -d "$VM_SERIAL_LOG" \
    -o ./serial.log \
    -t 120 \
    $LOGGING

WAIT_FOR_LOGIN_EXITCODE=$?

if [ "$OUTPUT" != "" ]; then
    mkdir -p $OUTPUT
    sudo cp ./serial.log $OUTPUT/serial.log
fi

if [ $WAIT_FOR_LOGIN_EXITCODE -ne 0 ]; then
    echo "Failed to deploy VM"
    exit $WAIT_FOR_LOGIN_EXITCODE
fi