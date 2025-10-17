# Local validation

## Build RPMs, Images, and tools

``` bash
TEST_NAME="auto-rollback"
TEST_IMAGES_FOLDER="../test-images"

# Build Trident RPMs
make bin/trident-rpms.tar.gz
make -C $TEST_IMAGES_FOLDER copy-trident-rpms
# Build install (regular.cosi) and update images (regular-2.cosi)
mkdir -p artifacts/test-image
rm -rf $TEST_IMAGES_FOLDER/build/trident-testimage.cosi
rm -rf artifacts/test-image/regular*.cosi
make -C $TEST_IMAGES_FOLDER trident-testimage
mv $TEST_IMAGES_FOLDER/build/trident-testimage.cosi artifacts/test-image/regular.cosi
make -C $TEST_IMAGES_FOLDER trident-testimage
cp $TEST_IMAGES_FOLDER/build/trident-testimage.cosi artifacts/test-image/regular-2.cosi

make bin/netlaunch
make bin/netlisten
```

## Create Trident Host Configuration

``` bash
mkdir -p input
cp tests/e2e_tests/trident_configurations/$TEST_NAME/trident-config.yaml input/trident.yaml
sed -i "s|sshPublicKeys: \[\]|sshPublicKeys: \[\"$(cat ~/.ssh/id_rsa.pub)\"\]|" input/trident.yaml
sed -i 's|/dev/disk/by-path/pci-0000:00:1f.2-ata-2|/dev/sda|' input/trident.yaml
sed -i 's|/dev/disk/by-path/pci-0000:00:1f.2-ata-3|/dev/sdb|' input/trident.yaml
echo "Using trident configuration:"
echo -e "$(cat input/trident.yaml)\n"
```

## Create test VM

``` bash
make bin/virtdeploy
./tools/virt-deploy create --mem 24 --disks 32,32
cp tools/vm-netlaunch.yaml input/netlaunch.yaml
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
ssh -o StrictHostKeyChecking=no -i ~/.ssh/id_rsa testing-user@$VM_IP sudo cat /var/lib/trident/trident-update-check-failure-*.log > failure.log
grep "failure for ab update" failure.log
```
