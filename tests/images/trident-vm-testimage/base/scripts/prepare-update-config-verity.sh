CONFIG_PATH=/var/lib/trident/update-config.yaml
DOWNLOAD_URL_PREFIX="http://192.168.122.1:8000"

cat <<EOF > $CONFIG_PATH
image:
  url: $DOWNLOAD_URL_PREFIX/verity.cosi
  sha384: ignored
EOF

INTERFACE_MASK='enp*'
if [ "$1" == "eth0" ]; then
  INTERFACE_MASK="$1"
fi

if [ "$1" != "uki" ]; then
  cat <<EOF >> $CONFIG_PATH
scripts:
  postConfigure:
    - name: rw-overlay
      runOn: [all]
      content: |
        mkdir -p /var/lib/trident-overlay/etc-rw/upper && mkdir -p /var/lib/trident-overlay/etc-rw/work
EOF
fi

cat <<EOF >> $CONFIG_PATH
internalParams:
  allowUnusedFilesystems: true
EOF

if [ "$1" == "uki" ]; then
  cat <<EOF >> $CONFIG_PATH
  uki: true
  disableGrubNoprefixCheck: true
EOF
fi
