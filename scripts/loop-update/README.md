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

- The servicing tests are backed by a storm scenario (../tools/storm/servicing).

- To run the scenario, you can use the `servicing-tests.sh` script or by invoking
  the storm binary directly.

- The servicing tests are composed of several storm test cases:

1. For Azure VMs, `publish-sig-image` is the first testcase and it will
   configure an appropriate qcow2 image as needed for an Azure VM and 
   upload it.
2. For all VMs, `deploy-vm` is the next phase and will create a VM on the
   selected platform.
3. For all VMs, `check-deployment` will verify that the VM has been started
   and that it booted from the expected volume.`
4. For all VMs, `update-loop` will update the VM the specified number of
   times, applying the update images and checking that the VM is in the 
   expected state.
5. For all VMs, `rollback` will validate that rollback and update works.
6. For all VMs, `collect-logs` will collect logs from the VM.
7. For all VMs, `cleanup-vm` will delete the VM.

