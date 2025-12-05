# Testing Manual Rollback and Runtime Updates

This storm scenario can be run locally by following the instructions below.

The test will do the following:

* Update the standard QCOW2 to include sysext extesion v1
* Start a VM with the updated QCOW2
* Verify extension is v1, active volume is A, and rollback info is as expected
* Run an A/B update that includes sysext extension v2
* Verify extension is v2, active volume is B, and rollback info is as expected
* Run a runtime update that includes sysext extension v3
* Verify extension is v3, active volume is B, and rollback info is as expected
* Run a runtime update that excludes sysext extension
* Verify extension does not exist, active volume is B, and rollback info is as expected
* Run manual rollback (of 2nd runtime update)
* Verify extension is v3, active volume is B, and rollback info is as expected
* Run manual rollback (of 1st runtime update)
* Verify extension is v2, active volume is B, and rollback info is as expected
* Run manual rollback (of A/B update)
* Verify extension is v1, active volume is A, and rollback info is as expected

Test can be configured to skip some testing:

* `--skip-runtime-updates` - skips testing associated with runtime update
* `--skip-manual-rollbacks` - skips testing associated with manual rollbacks
* `--skip-extension-testing` - skips testing associated with extensions

> Note: there are 2 test images (trident-vm-usr-verity-testimage and trident-vm-grub-verity-testimage) that can be used to run the tests, pick the desired image by setting `TEST_IMAGE_NAME` in the script below. If using trident-vm-grub-verity-testimage, extension testing will be skipped as Image Customizer cannot add an extension to the original QCOW2.

``` sh
# TEST_IMAGE_NAME="trident-vm-grub-verity-testimage"
TEST_IMAGE_NAME="trident-vm-usr-verity-testimage"

# Build storm-trident binary
make bin/storm-trident
# Build the test extensions
pushd ./artifacts
../bin/storm-trident script build-extension-images --build-sysexts --num-clones 3
popd
# Build the Trident rpms 
sudo rm bin/trident-rpms.tar.gz
sudo rm -rf bin/RPMS
make bin/trident-rpms.tar.gz
# Ensure that there are no previous test images that the test might pick up
sudo rm artifacts/trident-vm-*-testimage.qcow2 artifacts/trident-vm-*-testimage.cosi
# Build the required test images
make artifacts/$TEST_IMAGE_NAME.cosi
make artifacts/$TEST_IMAGE_NAME.qcow2

SKIP_FLAGS="--skip-runtime-updates"
if [ "$TEST_IMAGE_NAME" == "trident-vm-grub-verity-testimage" ]; then
  # skip extension testing
  SKIP_FLAGS="$SKIP_FLAGS --skip-extension-testing"
fi

sudo ./bin/storm-trident run rollback -w --verbose \
    --artifacts-dir ./artifacts/ \
    --output-path /tmp/output \
    --platform qemu \
    --ssh-private-key-path ./artifacts/id_rsa \
    --ssh-public-key-path ./artifacts/id_rsa.pub \
    $SKIP_FLAGS
 ```
