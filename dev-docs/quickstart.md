# Trident Quickstart Guide

The purpose of this guide is to provide the steps for running Trident in a local
developer setup. Once the setup is complete, you should be able to start
developing and testing with Trident in your local dev environment.

## Prerequisites

- Please ensure you have completed the necessary steps from the
  [Prerequisites](prerequisites.md) guide.
- You *may* need to [log in to the private Cargo registry](cargo-auth.md).

## Environment Setup

1. Follow the steps in the [virt-deploy
   guide](https://dev.azure.com/mariner-org/ECF/_git/argus-toolkit?version=GBmain&anchor=netlaunch-configuration&path=/virtdeploy/README.md)
   to set up the virtual machine that will be used to run Trident in the local
   environment.

2. Change directory to the Trident repository: `cd trident`.
3. Link the `vm-netlaunch.yaml` file generated in the virt-deploy step to the `input`
   directory in the Trident repository.

   ```bash
    mkdir input
    pushd input
    ln -s ../../argus-toolkit/vm-netlaunch.yaml netlaunch.yaml
    popd
    ```

4. Run the following command to download the latest images for Trident to use:
   `make download-runtime-partition-images`. This will download the latest
   images to the `artifacts/test-image` folder.

5. Create the starter host configuration file by running `make
   starter-configuration`. This will create the file `input/trident.yaml` with a
   basic configuration.

6. Add your SSH public key to the `input/trident.yaml` file. This will allow you
   to connect to the VM over SSH. Look for `sshPublicKeys: []` under the test
   user and add your key in the list. For example:

   ```yaml
    sshPublicKeys:
      - ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABgQDZ6...
    ```

7. Run `make run-netlaunch` to execute the Trident deployment. You should see
   the Trident deployment logs in the terminal.
   - Note: To watch the serial console logs for the VM, run `make
     watch-virtdeploy` in a different shell.

8. Once the deployment is complete, you can connect to the VM over `ssh` if it
   was provided in the Trident configuration file such as: `ssh
   user@192.168.242.2`.
    - Note: The IP of the VM by default is `192.168.242.2`, unless explicitly
    changed when creating the VM with `virt-deploy`.