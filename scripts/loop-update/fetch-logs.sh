#!/bin/bash
set -euo pipefail

. $(dirname $0)/common.sh

downloadJournalLog() {
    local DEST=$1

    local JOURNAL_LOG=/tmp/journal.log

    # Blocking error causing abort, so we can do other cleanup tasks
    set +e
    sshCommand "sudo journalctl --no-pager > $JOURNAL_LOG && sudo chmod 644 $JOURNAL_LOG"
    if [ $? -eq 0 ]; then
        scpDownloadFile $JOURNAL_LOG $DEST
    else
        echo "Failed to download journal log"
    fi
    set -e
}

scpDownloadFile() {
    local SRC=$1
    local DEST=$2

    scp -r -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null $SSH_USER@$VM_IP:$SRC $DEST
}

downloadCrashdumps() {
    local DEST="$1"

    local CRASHDUMP_DIR=/var/crash

    if sshCommand "ls $CRASHDUMP_DIR/*"; then
        echo "Crash files found on host"
        sshCommand "sudo mv $CRASHDUMP_DIR/* /tmp/crash && sudo chmod -R 644 /tmp/crash && sudo chmod +x /tmp/crash"
        scpDownloadFile "/tmp/crash/*" "$DEST/"
        tail -n 50 "$DEST/vmcore-dmesg.txt"
    else
        echo "No crash files found on host"
    fi
}

downloadAzureSerialLog() {
    local DEST="$1"

    az vm boot-diagnostics get-boot-log --name "$VM_NAME" --resource-group "$TEST_RESOURCE_GROUP" | sed 's/\\r\\n/\n/g' > "$DEST"
}

if [ "$TEST_PLATFORM" == "azure" ]; then
    downloadAzureSerialLog $1/serial.log
    if [ $VERBOSE == True ]; then
        cat $1/serial.log
    else
        echo "Serial log saved to $1/serial.log"
    fi
fi

VM_IP=`getIp`

downloadJournalLog $1/journal.log
downloadCrashdumps $1/
