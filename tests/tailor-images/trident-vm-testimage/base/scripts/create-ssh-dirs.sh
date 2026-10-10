# Move the SSH host keys off the read-only /etc directory, so that sshd can run.
SSH_VAR_DIR="/srv/etc/ssh/"
mkdir -p "$SSH_VAR_DIR"
