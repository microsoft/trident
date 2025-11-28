set -eux

NAME=trident-config.yaml

CONFIGS=$(find e2e_tests/trident_configurations -name $NAME)

TEMP_DIR=$(mktemp -d)

TEMP_FILE=$TEMP_DIR/$NAME
for CONFIG in $CONFIGS; do
  echo "Validating $TEMP_FILE from $CONFIG..."
  rm -f $TEMP_FILE
  cp $CONFIG $TEMP_FILE
  python3 .pipelines/templates/stages/testing_baremetal/update_host_config.py \
    --trident-yaml $TEMP_FILE \
    --oam-ip 127.0.0.2 \
    --interface-name myIfname \
    --oam-gateway "127.0.0.1" \
    --oam-mac "01:02:03:04:05:06"
  cargo run validate -c $TEMP_FILE -v debug
done
rm -rf $TEMP_DIR