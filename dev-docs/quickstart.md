# Trident Quickstart Guide

The purpose of this guide is to provide the steps for running Trident in a local
developer setup. Once the setup is complete, you should be able to start
developing and testing with Trident in your local dev environment.

## Prerequisites
Please ensure you have completed the necessary steps from the
[Prerequisites](prerequisites.md) guide.

## Environment Setup

1. Follow the steps in the [virt-deploy
   guide](https://dev.azure.com/mariner-org/ECF/_git/argus-toolkit?version=GBmain&anchor=netlaunch-configuration&path=/virtdeploy/README.md)
   to set up the virtual machine that will be used to run Trident in the local
   environment.

2. Change directory to the Trident repository: `cd trident`. Copy the
   vm-netlaunch.yaml file generated in the previous step to the `input`
   directory in the Trident repository. 
    - `mkdir input`
    - `cp ../argus-toolkit/vm-netlaunch.yaml input/netlaunch.yaml`

3. Run the following command to download the latest images for Trident to use:
   `make download-runtime-partition-images`. This will download the latest
   images to the `artifacts/test-image` folder.

4. Create the desired Trident configuration file `trident.yaml` in the `input`
   directory: `vi input/trident.yaml`. See the [Host
   Configuration](../docs/Reference/Host-Configuration.md) section for samples
   and more information if needed.
    - The image URLs should start with `http://NETLAUNCH_HOST_ADDRESS/files/`.
   It will use the files in `./artifacts/test-image` directory. To reference a
   file at `./artifacts/test-image/root.rawzst`, the URL would be
   `http://NETLAUNCH_HOST_ADDRESS/files/root.rawzst`.

5. Run `make run-netlaunch` to execute the Trident deployment. You should see
   the Trident deployment logs in the terminal.
   - Note: To watch the serial console logs for the VM, run `make
     watch-virtdeploy` in a different shell.

6. Once the deployment is complete, you can connect to the VM over `ssh` if it
   was provided in the Trident configuration file such as: `ssh
   user@192.168.242.2`.
    - Note: The IP of the VM by default is `192.168.242.2`, unless explicitly
    changed when creating the VM with `virt-deploy`.