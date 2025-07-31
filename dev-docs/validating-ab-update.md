# Manual A/B Update Validation Steps

The purpose of this document is to provide clear guidelines for developers on
how to manually validate the A/B update flow with Trident.

- [Manual A/B Update Validation Steps](#manual-ab-update-validation-steps)
  - [Validation steps](#validation-steps)
  - [Staging and Finalizing A/B Update](#staging-and-finalizing-ab-update)

## Validation steps

1. First, make the runtime OS image payload available for Trident to operate
   on. An easy way to do so is to use the following command:
   `make download-runtime-images`. This will download the latest Trident
   images to the `artifacts/test-image` folder: `regular.cosi`, `verity.cosi`,
   and `container.cosi`, which is the host OS image for container testing. For
   more details on container testing, please reference [Validating Container](/dev-docs/validating-container.md).

   The downloaded OS image can then be referenced in Host Configuration as
   follows: `http://NETLAUNCH_HOST_ADDRESS/files/<image_file_name>`. Netlaunch
   will substitute the placeholder with the actual IP address and serve the
   files from `artifacts/test-image` in the `files` sub-directory.

2. Then, update the Host Configuration in `input/trident.yaml` to include an
   `abUpdate` section and A/B volume pairs, so that A/B update is enabled. For
   example, in the Host  Configuration below, Trident is requested to create
   **two copies** of the `root` partition, i.e., an A/B volume pair with ID
   `root` that contains two partitions `root-a` and `root-b`.

   ```yaml
   image:
      url: http://NETLAUNCH_HOST_ADDRESS/files/regular.cosi
      sha384: ignored
   storage:
      disks:
         - id: os
            device: /dev/disk/by-path/pci-0000:00:1f.2-ata-2
            partitionTableType: gpt
            partitions:
               - id: root-a
                  type: root
                  size: 8G
               - id: root-b
                  type: root
                  size: 8G
               - id: esp
                  type: esp
                  size: 1G
               - id: swap
                  type: swap
                  size: 2G
               - id: home
                  type: home
                  size: 1G
               - id: trident
                  type: linux-generic
                  size: 1G
         - id: disk2
            device: /dev/disk/by-path/pci-0000:00:1f.2-ata-3
            partitionTableType: gpt
            partitions: []
      abUpdate:
         volumePairs:
            - id: root
               volumeAId: root-a
               volumeBId: root-b
      filesystems:
         - deviceId: swap
            type: swap
            source: new
         - deviceId: trident
            type: ext4
            source: new
            mountPoint: /var/lib/trident
         - deviceId: home
            type: ext4
            source: new
            mountPoint: /home
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
            content: echo 'testing-user ALL=(ALL:ALL) NOPASSWD:ALL' > /etc/sudoers.d/testing-user
   os:
      netplan:
         version: 2
         ethernets:
            vmeths:
            match:
               name: enp*
            dhcp4: true
      users:
         - name: testing-user
            sshPublicKeys: []
            sshMode: key-only
   ```

   Remember to also update the `sshPublicKeys` field with the correct key for
   your machine, so that you can later SSH into the VM.

3. For feature testing, the Host Configuration should be modified to contain
   RAID arrays, verity, encryption, etc., to ensure that the A/B upgrade flow
   succeeds with these special features enabled.

4. Boot the VM simulating a Bare Metal host with the Provisioning OS using the
   standard command `make run-netlaunch`.

5. When the clean install, i.e. the installation of the initial runtime OS is
   completed, log into the VM using SSH: `ssh root@<IP_address>`. The IP
   address and the port number of the VM will be exposed in the Netlaunch logs:

   `INFO[0019] Trident connected from <IP_address>:<port_number>`

6. Download images for A/B update. For GRUB to be able to correctly boot into
   the B partition, these images need to come from a different build, so that
   the ESP UUIDs are different. The easiest way to do this is to manually
   download the OS image payloads from a successful run of the Trident PR
   pipeline, i.e. `trident-pr-e2e`.

7. Make a copy of `input/trident.yaml` used for the clean install servicing and
   update the URL to point to the update OS image. An easy way to make the
   updated payload available is to use Netlisten to serve them at a local
   server for Trident to pull from.

   ```bash
   cp input/trident.yaml input/trident-update.yaml

   # Use an IDE or vim to update the URL inside the Host Configuration to
   # point to the updated image.
   # E.g. http://<VM_IP_address>:<any_port_number>/files/v2/regular.cosi

   # Build Netlisten
   make bin/netlisten

   # Run Netlisten to serve the images at the chose port number
   bin/netlisten -s artifacts/test-image -p <any_port_number>
   ```

8. Inside the VM, request an A/B update by running Trident.

   ```bash
   vim trident-update.yaml

   # Copy the updated HC from input/trident-update.yaml here

   # Re-run Trident
   sudo /usr/bin/trident update -v trace trident-update.yaml
   ```

9. Confirm that the VM simulating a BM host reboots into the new runtime OS
   image. SSH back into the host and view the changes to the system by fetching
   the Host Status with `trident get`. Specifically, make sure that
   `abActiveVolume` is set to `volume-b`; that the image URLs have been
   updated; and that there are no failures, i.e. `lastError` is empty.

10. You can view the full background log under `/var/log/trident-full.log`, as
   well as any log files persisted from the previous servicing, for more info.

11. You can also use commands such as `blkid` and `mount` to confirm that the
   volume B is mounted at root, as expected.

## Staging and Finalizing A/B Update

In addition to testing the standard A/B update flow, where the new OS images
are staged and then, immediately, finalized, it is also important to validate
the scenario where the deployment is staged and finalized separately. This can
be done with the `--allowed-operations` option in the following way:

- To only stage a new deployment, set `--allowed-operations stage`.
- To only finalize the staged deployment, set `--allowed-operations finalize`.
- To both stage a new deployment and then immediately finalize it, set
  `--allowed-operations stage,finalize`. This is the **default** value, so when
  the argument is not explicitly provided, the deployment will be both staged
  and immediately finalized.
