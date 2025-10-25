# Local validation

## Pick UEFI fallback mode

Choose one of the following modes by setting the `FALLBACK_MODE` variable in the script below:

``` bash
# Expectation: Boot from B when UEFI is corrupted
#   FALLBACK_MODE="rollforward"
# Expectation: Boot from A when UEFI is corrupted
#   FALLBACK_MODE="rollback"
# Expectation: Broken boot
#   FALLBACK_MODE="none"
FALLBACK_MODE="rollback"
```

## Build RPMs, Images, and tools

``` bash
TEST_NAME="uefifallback"
TEST_IMAGES_FOLDER="../test-images"

# Build Trident RPMs
make bin/trident-rpms.tar.gz
make -C $TEST_IMAGES_FOLDER copy-trident-rpms
# Build install (regular.cosi) and update images (regular_v2.cosi)
mkdir -p artifacts/test-image
make -C $TEST_IMAGES_FOLDER trident-testimage
mv $TEST_IMAGES_FOLDER/build/trident-testimage.cosi artifacts/test-image/regular.cosi
make -C $TEST_IMAGES_FOLDER trident-testimage
cp $TEST_IMAGES_FOLDER/build/trident-testimage.cosi artifacts/test-image/regular_v2.cosi

make tools/netlaunch
make tools/netlisten
make bin/storm-trident
```

## Create Trident Host Configuration

``` bash
mkdir -p input
cp tests/e2e_tests/trident_configurations/$TEST_NAME/trident-config.yaml input/trident.yaml
sed -i "s|sshPublicKeys: \[\]|sshPublicKeys: \[\"$(cat ~/.ssh/id_rsa.pub)\"\]|" input/trident.yaml
sed -i 's|/dev/disk/by-path/pci-0000:00:1f.2-ata-2|/dev/sda|' input/trident.yaml
sed -i 's|/dev/disk/by-path/pci-0000:00:1f.2-ata-3|/dev/sdb|' input/trident.yaml
sed -i "s|uefiFallback: .*|uefiFallback: $FALLBACK_MODE|" input/trident.yaml

./bin/trident validate ./input/trident.yaml
echo "Valid host configuration? $?"
```

## Create test VM and run initial image installation

``` bash
make tools/virt-deploy
./tools/virt-deploy create --mem 24 --disks 32,32
make run-netlaunch
```

## Run update and validate

``` bash
# Get VM IP address
VM_IP=$(virsh domifaddr virtdeploy-vm-0 | grep ipv4 | awk '{print $4}' |  cut -d "/" -f1)
echo "VM_IP: $VM_IP"
# Get Port used for netlaunch/netlisten
ssh -o StrictHostKeyChecking=no -i ~/.ssh/id_rsa testing-user@$VM_IP "trident get configuration" > input/deployed-trident.yaml
NETLAUNCH_PORT=$(cat input/deployed-trident.yaml | grep image -A 2 | grep url | cut -d "/" -f 3 | cut -d ":" -f2)
echo "NETLAUNCH_PORT: $NETLAUNCH_PORT"
# Start netlisten to serve update image
./bin/netlisten --port $NETLAUNCH_PORT --servefolder artifacts/test-image > ./update.log 2>&1 &
# Run AB update with forced rollback using storm-trident
SSH_KEY="$HOME/.ssh/id_rsa"
echo "Using SSH Key: $SSH_KEY"
./bin/storm-trident helper ab-update -w \
    "$SSH_KEY" \
    "$VM_IP" \
    "testing-user" \
    "host" \
    --trident-config "/var/lib/trident/config.yaml" \
    --version 2 \
    --stage-ab-update \
    --finalize-ab-update \
    --expect-failed-commit

EXPECTED_VOLUME="volume-a"
if [ "$FALLBACK_MODE" == "rollforward" ]; then
    EXPECTED_VOLUME="volume-b"
fi

# Verify results
pushd tests/e2e_tests
python3 -u -m pytest -m uefifallback \
    -capture=no \
    --host "$VM_IP" \
    --runtime-env "host" \
    --configuration "./trident_configurations/$TEST_NAME" \
    --ab-active-volume "$EXPECTED_VOLUME" \
    --keypath "$HOME/.ssh/id_rsa" \
    -s
popd
```
