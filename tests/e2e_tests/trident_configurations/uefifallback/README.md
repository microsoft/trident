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
FALLBACK_MODE="rollforward"
```

## Build RPMs, Images, and tools

``` bash
TEST_NAME="uefifallback"
TEST_IMAGES_FOLDER="../test-images"

# Build Trident RPMs
make bin/trident-rpms.tar.gz
make -C $TEST_IMAGES_FOLDER copy-trident-rpms
# Build install (regular.cosi) and update images (regular-2.cosi)
mkdir -p artifacts/test-image
make -C $TEST_IMAGES_FOLDER trident-testimage
mv $TEST_IMAGES_FOLDER/build/trident-testimage.cosi artifacts/test-image/regular.cosi
make -C $TEST_IMAGES_FOLDER trident-testimage
cp $TEST_IMAGES_FOLDER/build/trident-testimage.cosi artifacts/test-image/regular-2.cosi

make tools/netlaunch
make tools/netlisten
```

## Create Trident Host Configuration

``` bash
mkdir -p input
cp tests/e2e_tests/trident_configurations/$TEST_NAME/trident-config.yaml input/trident.yaml
sed -i "s|sshPublicKeys: \[\]|sshPublicKeys: \[\"$(cat ~/.ssh/id_rsa.pub)\"\]|" input/trident.yaml
sed -i 's|/dev/disk/by-path/pci-0000:00:1f.2-ata-2|/dev/sda|' input/trident.yaml
sed -i 's|/dev/disk/by-path/pci-0000:00:1f.2-ata-3|/dev/sdb|' input/trident.yaml
sed -i "s|uefiFallback: .*|uefiFallback: $FALLBACK_MODE|" input/trident.yaml
echo "Using trident configuration:"
echo -e "$(cat input/trident.yaml)\n"
```

## Create test VM

``` bash
make tools/virt-deploy
./tools/virt-deploy create --mem 24 --disks 32,32
```

## Run initial image installation

``` bash
make run-netlaunch
```

## Create Update Trident Host Configuration

``` bash
VM_IP=$(virsh domifaddr virtdeploy-vm-0 | grep ipv4 | awk '{print $4}' |  cut -d "/" -f1)
echo "VM_IP: $VM_IP"
ssh -o StrictHostKeyChecking=no -i ~/.ssh/id_rsa testing-user@$VM_IP "trident get configuration" > input/trident-2.yaml
sed -i 's|regular.cosi|regular-2.cosi|' ./input/trident-2.yaml
sed -i 's|^  sha384: .*|  sha384: ignored|' ./input/trident-2.yaml
echo "Using updated trident configuration:"
echo -e "$(cat input/trident-2.yaml)\n"
```

## Start netlisten and run update

``` bash
NETLAUNCH_PORT=$(cat input/trident-2.yaml | grep image -A 2 | grep url | cut -d "/" -f 3 | cut -d ":" -f2)
echo "NETLAUNCH_PORT: $NETLAUNCH_PORT"
./bin/netlisten --port $NETLAUNCH_PORT --servefolder artifacts/test-image > ./update.log 2>&1 &

scp -o StrictHostKeyChecking=no -i ~/.ssh/id_rsa ./input/trident-2.yaml testing-user@$VM_IP:/tmp/trident.yaml
ssh -o StrictHostKeyChecking=no -i ~/.ssh/id_rsa testing-user@$VM_IP sudo trident update /tmp/trident.yaml
```

## Verify results

``` bash
if [[ "$FALLBACK_MODE" == "none" ]]; then
    echo "`virsh console virtdeploy-vm-0` should show a boot failure"
else
    ssh -o StrictHostKeyChecking=no -i ~/.ssh/id_rsa testing-user@$VM_IP sudo trident get status > status.yaml
    if [[ "$FALLBACK_MODE" == "rollforward" ]]; then
        if ! grep 'abActiveVolume: volume-b' status.yaml; then
            echo "ERROR: Did not boot from B as expected"
        fi
    elif [[ "$FALLBACK_MODE" == "rollback" ]]; then
        if ! grep 'abActiveVolume: volume-a' status.yaml; then
            echo "ERROR: Did not boot from A as expected"
        fi
    fi
fi
```
