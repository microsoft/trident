# Manual container validation steps

The purpose of this document is to provide a manual procedure for deploying and
validating an image running Trident from a container.

## Steps for clean install with Trident container image

1. Download the container installer ISO from Trident's artifacts feed by running
   `make download-trident-container-installer-iso`. This is the ISO from which
   the provisioning/management OS will run.

2. (Optionally) Download the Trident container image from Trident's artifacts
   feed by running `make artifacts/test-image/trident-container.tar.gz`. Note
   that this step is optional since the Trident container image will be
   downloaded as part of the command you run in Step 7.

3. Download the runtime OS images from Trident's artifacts feed by running `make
   download-runtime-images` so that we can deploy the container COSI file
   `container.cosi` with Trident. The OS images will be downloaded to
   `artifacts/test-images` and can be used by Trident for clean install
   deployment. `regular.cosi` and `verity.cosi` already have Trident RPMs baked
   into them. Meanwhile `container.cosi` is a light-weight host OS image
   developed specifically for container testing, so it does not contain the
   container image with the Trident bits (i.e. `trident-container.tar.gz`).

4. Create or update your Host Configuration in `input/trident.yaml`. For
   example, the YAML file below sets up a machine with two partitions using
   `container.cosi`:

   ```yaml
   image:
      url: http://NETLAUNCH_HOST_ADDRESS/files/container.cosi
      sha384: ignored
   storage:
      disks:
         - id: os
            device: /dev/sda
            partitionTableType: gpt
            partitions:
               - id: esp
                 type: esp
                 size: 1G
               - id: root
                 type: root
                 size: 8G
         filesystems:
            - deviceId: esp
              type: vfat
              mountPoint:
              path: /boot/efi
              options: umask=0077
            - deviceId: root
              type: ext4
              mountPoint: /
   scripts:
      postConfigure:
         - name: testing-privilege
           runOn:
           - clean-install
           - ab-update
           content: echo "testing-user ALL=(ALL:ALL) NOPASSWD:ALL" > /etc/sudoers.d/testing-user
   os:
      netplan:
         version: 2
         ethernets:
            vmeths:
            match:
               name: enp*
            dhcp4: true
      users:
         - name: root
           sshPublicKeys: []
           sshMode: key-only
   ```

   Remember to update the `sshPublicKeys` in order to be able to later SSH into
   the VM.

5. If you are using `container.cosi` for your testing image, it is necessary to
   copy over the `trident-container.tar.gz` file from the Provisioning OS to the
   Runtime OS via `additionalFiles`. Note that `trident-container.tar.gz` is the
   container image with Trident installed into it. This is because
   `trident-container.tar.gz` is not inside the OS image (`container.cosi`).
   This step is also necessary if you are using your own OS image that does not
   contain Trident. Make sure to update the `os` section of your Host
   Configuration as follows:

   ```yaml
   os:
      additionalFiles:
         - source: "/var/lib/trident/trident-container.tar.gz"
           destination: "/var/lib/trident/trident-container.tar.gz"
      ...
   ```

   This step can be skipped if your testing host OS image (i.e. alternative to
   `container.cosi`) already contains Trident.

6. Create a VM from the `argus-toolkit` repository as follows:

   ```bash
   ./virt-deploy create --mem 11
   ```

   Note that at least 11GB of RAM is necessary to run Trident in a container,
   since the Provisioning OS is allocated to use part of the RAM instead of the
   disk.

7. Boot the VM simulating a Bare Metal host with Netlaunch using `make
   run-netlaunch-container-images`. (Note: this is different from `make
   run-netlaunch`, which is used for the purposes of running Trident in
   non-container scenarios.)

In order to test an A/B Update flow, follow the directions in [Validating A/B
Update](/dev-docs/validating-ab-update.md) from step 7, while referencing the
[ReadMe](../README.md#running-from-container) for more details on running
Trident from a container.

### Alternative to using artifacts feed ISO and container image

Instead of using the provisioning OS ISO and container image from Trident's
artifacts feed, it is also possible to build your own custom ISO and container
image. You can build the Trident RPMs for use in a container by running `make
bin/trident-rpms/azl3.tar.gz`. Then, build your own container image by including
the Trident rpm in it as well as other necessary dependencies. See
[Dockerfile.runtime](../Dockerfile.runtime) for an example of how to do this.
You can publish your Trident container image to a registry. For example, you can
set up a local registry by doing the following (don't forget to update your
alias and registry):

```bash
ALIAS=<alias>
REGISTRY=<registry>
docker tag docker.io/trident/trident:latest $REGISTRY/trident:$ALIAS
docker push $REGISTRY/trident:$ALIAS
```

Laslty, ensure that the following directories are mounted as in the example
below (taken from
[trident-container.service](https://dev.azure.com/mariner-org/ECF/_git/test-images?path=%2Fplatform-integration-images%2Ftrident-container-installer-testimage%2Fbase%2Ffiles%2Ftrident-container.service&version=GBmain&_a=contents))
in order for the container to run Trident successfully:

```bash
docker run --name trident_container --rm --privileged -v /etc/trident:/etc/trident -v /run/initramfs/live:/trident_cdrom -v /var/lib/trident:/var/lib/trident -v /var/log:/var/log -v /:/host -v /dev:/dev -v /run:/run -v /sys:/sys --pid host --ipc host trident/trident:latest run
```
