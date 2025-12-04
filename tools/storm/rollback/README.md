This scenario can be run locally.

> Note: there are 2 test images (trident-vm-usr-verity-testimage and trident-vm-grub-verity-testimage) that can be used to run the tests, pick the desired image by setting `TEST_IMAGE_NAME` in the script below.

``` sh
# TEST_IMAGE_NAME="trident-vm-grub-verity-testimage"
TEST_IMAGE_NAME="trident-vm-usr-verity-testimage"


make bin/trident-rpms.tar.gz
# Ensure that there are no previous test images that the test might pick up
sudo rm artifacts/trident-vm-*-testimage.qcow2 artifacts/trident-vm-*-testimage.cosi
# Build the required test images
make artifacts/$TEST_IMAGE_NAME.cosi
make artifacts/$TEST_IMAGE_NAME.qcow2

SKIP_FLAGS=""
if [ "$TEST_IMAGE_NAME" == "trident-vm-grub-verity-testimage" ]; then
  # skip extension testing
  SKIP_FLAGS="--skip-extension-testing"
fi

sudo ./bin/storm-trident run rollback -w --verbose \
    --artifacts-dir ./artifacts/ \
    --output-path /tmp/output \
    --platform qemu \
    --ssh-private-key-path ./artifacts/id_rsa \
    --ssh-public-key-path ./artifacts/id_rsa.pub \
    --skip-runtime-updates \
    $SKIP_FLAGS
 ```
