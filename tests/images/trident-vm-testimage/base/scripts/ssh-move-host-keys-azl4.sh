#!/bin/bash
# AZL4-compatible variant of ssh-move-host-keys.sh.
#
# AZL3 sshd reads the main /etc/ssh/sshd_config and we appended HostKey
# lines to it. AZL4 sshd 10.0+ supports drop-ins under /etc/ssh/sshd_config.d/
# which is the cleaner approach.
SSH_VAR_DIR="/var/srv/etc/ssh"
mkdir -p /etc/ssh/sshd_config.d
cat > /etc/ssh/sshd_config.d/50-trident-host-keys.conf <<EOC
HostKey ${SSH_VAR_DIR}/ssh_host_rsa_key
HostKey ${SSH_VAR_DIR}/ssh_host_ecdsa_key
HostKey ${SSH_VAR_DIR}/ssh_host_ed25519_key
EOC
