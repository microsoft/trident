#!/usr/bin/env bash
set -euo pipefail

marker_file="${TRIDENT_ACL_AGENT_TESTER_REBOOT_MARKER:-./trident-acl-agent-reboot-signal}"
self_name="$(basename "$0")"

if [[ "$self_name" == "systemctl" && "${1:-}" != "reboot" ]]; then
    exec /usr/bin/systemctl "$@"
fi

mkdir -p "$(dirname "$marker_file")"
printf 'reboot-requested\n' > "$marker_file"
exit 0
