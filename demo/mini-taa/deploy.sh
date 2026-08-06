#!/usr/bin/env bash
#
# Deploy the mini-TAA (trident-acl-agent) into the ACL demo VM.
#
# Places the binary + systemd unit on the guest's SHARED, PERSISTENT ext4 root
# (/var and /etc are NOT part of the A/B swap), so both survive the update and
# the reboot. `systemctl enable` therefore also persists, giving us the closing
# "agent returns to no-update" beat automatically after the reboot.
#
# Usage:
#   ./deploy.sh <path-to-binary> [user@host] [ssh-key]
#
# Defaults target the demo VM described by the demo lead.
set -euo pipefail

BINARY="${1:?path to trident-acl-agent binary required}"
TARGET="${2:-azureuser@192.168.122.77}"
SSH_KEY="${3:-$HOME/.ssh/id_rsa}"

UNIT_SRC="$(dirname "$0")/trident-acl-agent.service"
INSTALL_DIR="/var/lib/trident-acl-agent"
BIN_DEST="${INSTALL_DIR}/trident-acl-agent"

ssh_cmd=(ssh -i "$SSH_KEY" -o StrictHostKeyChecking=no "$TARGET")
scp_cmd=(scp -i "$SSH_KEY" -o StrictHostKeyChecking=no)

echo ">> Creating ${INSTALL_DIR} on ${TARGET}"
"${ssh_cmd[@]}" "sudo mkdir -p ${INSTALL_DIR}"

echo ">> Copying binary and unit to a staging area"
"${scp_cmd[@]}" "$BINARY" "${TARGET}:/tmp/trident-acl-agent"
"${scp_cmd[@]}" "$UNIT_SRC" "${TARGET}:/tmp/trident-acl-agent.service"

echo ">> Installing binary and unit"
"${ssh_cmd[@]}" "sudo install -m 0755 /tmp/trident-acl-agent ${BIN_DEST} \
  && sudo install -m 0644 /tmp/trident-acl-agent.service /etc/systemd/system/trident-acl-agent.service \
  && sudo systemctl daemon-reload \
  && sudo systemctl enable --now trident-acl-agent.service \
  && rm -f /tmp/trident-acl-agent /tmp/trident-acl-agent.service"

echo ">> Done. Follow the agent live with:"
echo "   ${ssh_cmd[*]} 'journalctl -u trident-acl-agent -f'"
