# Testing Manual Rollback in e2e tests

# Local validation

## Pick UEFI fallback mode

Choose one of the following modes by setting the `FALLBACK_MODE` variable in the script below:

``` bash

## Build RPMs, Images, and tools
TEST_IMAGES_FOLDER="../test-images"

TEST_NAME="combined"
IMAGE_NAME="trident-usrverity-testimage"
COSI_NAME="usrverity"

# Build Trident RPMs
sudo rm -rf bin/trident-rpms.tar.gz
sudo rm -rf bin/RPMS
make bin/trident-rpms.tar.gz
sudo make -C $TEST_IMAGES_FOLDER copy-trident-rpms
# Build install (regular.cosi) and update images (regular_v2.cosi)
mkdir -p artifacts/test-image
pushd $TEST_IMAGES_FOLDER
python3 ./testimages.py build $IMAGE_NAME --clones 2
popd
sudo mv $TEST_IMAGES_FOLDER/build/${IMAGE_NAME}_0.cosi artifacts/test-image/$COSI_NAME.cosi
sudo mv $TEST_IMAGES_FOLDER/build/${IMAGE_NAME}_1.cosi artifacts/test-image/${COSI_NAME}_v2.cosi
sudo mv $TEST_IMAGES_FOLDER/build/ca_cert.pem artifacts/test-image/ca_cert.pem

## Build netlaunch and netlisten
make tools/netlaunch
make tools/netlisten

## Create Trident Host Configuration
mkdir -p input
cp tests/e2e_tests/trident_configurations/$TEST_NAME/trident-config.yaml input/trident.yaml
sed -i "s|sshPublicKeys: \[\]|sshPublicKeys: \[\"$(cat ~/.ssh/id_rsa.pub)\"\]|" input/trident.yaml
sed -i 's|/dev/disk/by-path/pci-0000:00:1f.2-ata-2|/dev/sda|' input/trident.yaml
sed -i 's|/dev/disk/by-path/pci-0000:00:1f.2-ata-3|/dev/sdb|' input/trident.yaml
./bin/trident validate ./input/trident.yaml
echo "Valid host configuration? $?"

## Create test VM and run initial image installation
make tools/virt-deploy
./tools/virt-deploy create --mem 24 --disks 32,32

# Build storm
make bin/storm-trident

## Run clean install using netlaunch
sudo chown $USER:$USER ./artifacts/test-image/ca_cert.pem
./bin/netlaunch \
    --iso ./bin/trident-mos.iso \
    --config ./input/netlaunch.yaml \
    --trident ./input/trident.yaml \
    --servefolder ./artifacts/test-image \
    --logstream \
    --trace-file /tmp/trident-clean-install-metrics.jsonl \
    --force-color \
    --full-logstream /tmp/logstream-full.log \
    --wait-for-provisioned-state \
    --secure-boot \
    --signing-cert ./artifacts/test-image/ca_cert.pem

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

# A/B update
./bin/storm-trident helper ab-update -w \
    "$SSH_KEY" \
    "$VM_IP" \
    "testing-user" \
    "host" \
    --trident-config "/var/lib/trident/config.yaml" \
    --version 2 \
    --stage-ab-update \
    --finalize-ab-update

# Start netlisten to provide phone home receiver
./bin/netlisten --port $NETLAUNCH_PORT --servefolder artifacts/test-image > ./rollback.log 2>&1 &

# Test rollback
./bin/storm-trident helper manual-rollback -w \
    "$SSH_KEY" \
    "$VM_IP" \
    "testing-user" \
    "host"
```
