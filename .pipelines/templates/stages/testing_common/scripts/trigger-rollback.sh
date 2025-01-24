#!/bin/bash
# trigger-rollback.sh

set -eux

# Only trigger rollback on the first run of the script. This is necessary so that we can test
# whether Trident can re-run A/B update successfully with the same HC after a rollback.
if [ ! -f "$EXEC_ROOT/var/already-run" ]; then
    touch "$EXEC_ROOT/var/already-run"

    # Define the service file path
    SERVICE_NAME="one-time-reboot.service"
    SERVICE_FILE="/etc/systemd/system/${SERVICE_NAME}"

    # Create the systemd service file
    cat <<EOF > "$SERVICE_FILE"
[Unit]
Description=One-time reboot service
# Should run before Trident
Before=trident.service

[Service]
Type=oneshot
# Disable the service first
ExecStartPre=systemctl disable ${SERVICE_NAME}
ExecStart=systemctl reboot

[Install]
WantedBy=multi-user.target
EOF

    # Enable the service to run at next boot
    echo "Enabling ${SERVICE_NAME}..."
    systemctl enable "$SERVICE_NAME"
fi
