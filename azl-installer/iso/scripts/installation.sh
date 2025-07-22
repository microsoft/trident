#!/bin/bash
set -ex
trap '/bin/bash' ERR

CD_INSTALLER_DIR="/mnt/trident_cdrom/installer"
WORKING_DIR="/root/installer"
TRIDENT_CONFIG="/etc/trident/config.yaml"
TRIDENT_SCRIPTS="/etc/trident/scripts"
TRIDENT_PASSWORD_SCRIPT="$TRIDENT_SCRIPTS/user-password.sh"

cp -r "$CD_INSTALLER_DIR/" "/root/"

cd "$WORKING_DIR"
"$WORKING_DIR/liveinstaller" \
  --build-dir=$WORKING_DIR/ \
  --attended \
  --template-config=$WORKING_DIR/attended_config.json \
  --log-level=trace \
  --log-file=$WORKING_DIR/liveinstaller.log > "$WORKING_DIR/output_liveinstaller.log" 2>&1

# Update device path in Trident's Host Configuration with user input
DISK=$(jq -r .disk_path "$WORKING_DIR/user_input.json")
sed -i "s|__DISK_PATH__|$DISK|g" "$TRIDENT_CONFIG"

# Update hostname in Trident's Host Configuration with user input
HOSTNAME=$(jq -r .hostname "$WORKING_DIR/user_input.json")
sed -i "s|__HOST_NAME__|$HOSTNAME|g" "$TRIDENT_CONFIG"

# Update username in Trident's Host Configuration with user input
USERNAME=$(jq -r .username "$WORKING_DIR/user_input.json")
sed -i "s|__USER_NAME__|$USERNAME|g" "$TRIDENT_CONFIG"

# Create user-password.sh script to set the password for the user
mkdir -p $TRIDENT_SCRIPTS
USERPASSWORD=$(jq -r .password "$WORKING_DIR/user_input.json")
echo "echo '$USERNAME:$USERPASSWORD' | chpasswd" > $TRIDENT_PASSWORD_SCRIPT
chmod 700 $TRIDENT_PASSWORD_SCRIPT

/bin/trident install
/bin/bash