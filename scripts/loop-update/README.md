# Update looping helper scripts

The purpose of these scripts is to simplify the process of looping through A/B
updates on an image.

The scripts are consumed by the `vm-servicing` stage of the e2e pipeline and by
the scaling pipeline. They can be also used locally.

## Usage

- Set `TEST_PLATFORM` environment variable to `qemu` or `azure` to select the
  target platform.

- `rebuild-images.sh`: Uses `../test-images` to build a base image for the VM
  servicing and two sets of update images. The produced images are moved to
  `artifacts`. Generally needs to be only rerun when you want to refresh the
  images to be used.
  
- For Azure VMs, you will need to publish the image once, before it could be
  used to create VMs. To publish the image, call `publish-sig-image.sh`.

- `deploy-vm.sh`: Creates a VM instance with the base image and starts the VM.
  It ensures the VM gets to the login prompt.

- `check-deployment.sh`: Fetches the Host Status of the freshly deployed VM to
  ensure it is in an expected state. You need to deploy the VM first using the
  script above.

- `loop-update.sh`: Loops through the update images and applies them to the VM.
  It ensures the VM gets to the login prompt after each update and confirms the
  Host Status is as expected. This script will power off and restart the VM
  every 10 runs. By default, it will execute 20 loops, and you can change this
  by setting `RETRY_COUNT` environment variable.

- `cleanup-vm.sh`: Deletes the VM instance. The deploy scripts will delete
  automatically before creating the new VMs. The other scripts use a presence of
  a QEMU VM to decide whether to target Azure or QEMU, so if you are switching
  between the two, you may need to delete the VM using this script first.

- `common.sh`: Not used directly. Contains common functions used by the other
  scripts.
