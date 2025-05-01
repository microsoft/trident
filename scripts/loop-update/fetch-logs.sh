#!/bin/bash
set -euo pipefail

. $(dirname $0)/common.sh
OUTPUT_DIR="$1"

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

    scp -i ../test-images/build/id_rsa -r -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null $SSH_USER@$VM_IP:$SRC $DEST
}

downloadCrashdumps() {
    local DEST="$1"

    local CRASHDUMP_DIR=/var/crash

    if sshCommand "ls $CRASHDUMP_DIR/*"; then
        echo "Crash files found on host"
        adoError "Crash files found on host"
        sshCommand "sudo mv $CRASHDUMP_DIR/* /tmp/crash && sudo chmod -R 644 /tmp/crash && sudo chmod +x /tmp/crash"
        scpDownloadFile "/tmp/crash/*" "$DEST/"
        tail -n 50 "$DEST/vmcore-dmesg.txt"
    else
        echo "No crash files found on host"
    fi
}

downloadAzureSerialLog() {
    local DEST="$1"

    # Output of az vm boot-diagnostics get-boot-log is not very readable, so
    # clean it up a bit:
    # - convert \r\n to newlines
    # - remove unicode characters
    # - remove lines with only quotes
    # - remove lines with only dashes
    # - remove empty lines
    az vm boot-diagnostics get-boot-log --name "$VM_NAME" --resource-group "$TEST_RESOURCE_GROUP" | sed -r 's/\\r\\n/\n/g;s/\\u[a-z0-9]{4}//g;/^"$/d;/^-+$/d;/^$/d' > "$DEST"
}

if [ "$TEST_PLATFORM" == "azure" ]; then
    downloadAzureSerialLog $OUTPUT_DIR/serial.log
    if [ $VERBOSE == True ]; then
        cat $OUTPUT_DIR/serial.log
    else
        echo "Serial log saved to $OUTPUT_DIR/serial.log"
    fi
    analyzeSerialLog $OUTPUT_DIR/serial.log
fi

VM_IP=`getIp`

downloadJournalLog $OUTPUT_DIR/journal.log
downloadCrashdumps $OUTPUT_DIR/
