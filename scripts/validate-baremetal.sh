set -eux

NAME=trident-config.yaml

CONFIGS=$(find e2e_tests/trident_configurations -name $NAME)

TEMP_DIR=$(mktemp -d)

DEFAULT_INTERFACE_NAME=$(ip route show default | awk '/default/ {print $5}')

echo "myssh-key" > $TEMP_DIR/mysshkey

TEMP_FILE=$TEMP_DIR/$NAME
for CONFIG in $CONFIGS; do
  echo "Validating $TEMP_FILE from $CONFIG..."
  rm -f $TEMP_FILE
  cp $CONFIG $TEMP_FILE
  python3 .pipelines/templates/stages/deployment_testing/baremetal/update_host_config.py \
    --trident-yaml $TEMP_FILE \
    --iso-httpd-ip 127.0.0.1 \
    --oam-ip 127.0.0.2 \
    --ssh-pub-key \
    $TEMP_DIR/mysshkey \
    --interface-name ifname \
    --host-interface "$DEFAULT_INTERFACE_NAME" \
    --netlisten-port 8080
  cargo run validate -c $TEMP_FILE -v debug
done
rm -rf $TEMP_DIR