#!/bin/bash
set -ex
trap '/bin/bash' ERR

CD_INSTALLER_DIR="/mnt/trident_cdrom/installer"
WORKING_DIR="/root/installer"
TRIDENT_CONFIG="/etc/trident/config.yaml"

cp -r "$CD_INSTALLER_DIR/" "/root/"

# Liveinstaller currently fails during run, so we need to ignore the error
trap - ERR
set +e
cd "$WORKING_DIR"
"$WORKING_DIR/liveinstaller" \
  --input=$WORKING_DIR/imager_config.json \
  --imager=$WORKING_DIR/imager \
  --build-dir=$WORKING_DIR/ \
  --attended \
  --template-config=$WORKING_DIR/attended_config.json \
  --log-level=trace \
  --log-file=$WORKING_DIR/liveinstaller.log > "$WORKING_DIR/output_liveinstaller.log" 2>&1
set -e
trap '/bin/bash' ERR

USERNAME=$(jq -r .username "$WORKING_DIR/userinput.json")
USERPASSWORD=$(jq -r .password "$WORKING_DIR/userinput.json")

sed -i "s/###%%%@@@/$USERNAME/g" "$TRIDENT_CONFIG"
sed -i "s/%%%###@@@/$USERPASSWORD/g" "$TRIDENT_CONFIG"

/bin/trident install
/bin/bash
