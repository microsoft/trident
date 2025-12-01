CONFIG_PATH=/var/lib/trident/update-config.yaml
DOWNLOAD_URL_PREFIX="http://192.168.122.1:8000"

cp /etc/trident/config.yaml $CONFIG_PATH
cat <<EOF >> $CONFIG_PATH
image:
  url: $DOWNLOAD_URL_PREFIX/usrverity.cosi
  sha384: ignored
EOF