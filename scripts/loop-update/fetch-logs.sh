#/bin/bash

set -euo pipefail

. $(dirname $0)/common.sh

VM_IP=`getIp`

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

    scp -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null $SSH_USER@$VM_IP:$SRC $DEST
}

downloadJournalLog $1/journal.log
