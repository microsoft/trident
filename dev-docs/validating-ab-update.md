# Manual A/B Update Validation Steps

The purpose of this document is to provide clear guidelines for developers on
how to manually validate the A/B update flow with Trident.

- [Manual A/B update validation steps](#manual-ab-update-validation-steps)
  - [Validation Steps](#validation-steps)
  - [Staging and Finalizing A/B Update](#staging-and-finalizing-ab-update)

## Validation steps

1. First, make the runtime OS image payloads available for Trident to operate
   on. An easy way to do so is to use the following command:
   `make download-runtime-partition-images`. This will download the latest
   images to the `artifacts/test-image` folder. Then, the payload can be
   referenced as `http://NETLAUNCH_HOST_ADDRESS/files/<payload_name>` in the
   Host Configuration: Netlaunch will substitute the placeholder with the
   actual IP address and serve the files from `artifacts/test-image` in the
   `files` sub-directory at this address.

2. Then, update the Host Configuration in `input/trident.yaml` to include A/B
   volume pairs, so that A/B update is enabled. For example, in the Host
   Configuration below, Trident is requested to create **two copies** of the
   `root` partition, i.e., an A/B volume pair with ID `root` that contains two
   partitions `root-a` and `root-b`.

   ```yaml
   storage:
      disks:
      - id: os
         device: /dev/disk/by-path/pci-0000:00:1f.2-ata-2
         partitionTableType: gpt
         partitions:
            - id: esp
            type: esp
            size: 1G
            - id: root-a
            type: root
            size: 8G
            - id: root-b
            type: root
            size: 8G
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
      - deviceId: trident
         type: ext4
         mountPoint: /var/lib/trident
      - deviceId: home
         type: ext4
         mountPoint: /home
      - deviceId: esp
         type: vfat
         source:
            type: image
            url: http://NETLAUNCH_HOST_ADDRESS/files/esp.rawzst
            sha256: ignored
            format: raw-zst
         mountPoint:
            path: /boot/efi
            options: umask=0077
      - deviceId: root
         type: ext4
         source:
            type: image
            url: http://NETLAUNCH_HOST_ADDRESS/files/root.rawzst
            sha256: ignored
            format: raw-zst
         mountPoint: /
   scripts:
      postConfigure:
      - name: testing-privilege
         runOn:
            - clean-install
            - ab-update
         content: echo 'testing-user ALL=(ALL:ALL) NOPASSWD:ALL' > /etc/sudoers.d/testing-user
   os:
      network:
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

3. For feature testing, the Host Configuration should be modified to contain
   RAID arrays, verity, encryption, etc., to ensure that the A/B upgrade flow
   succeeds with these special features enabled.

4. Boot the VM simulating a Bare Metal host with the Provisioning OS using the
   standard command `make run-netlaunch`. Remember to update the `sshPublicKeys`
   field with the correct key for your machine, so that you can later SSH into
   the VM.

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
   update the URLs to point to the new images. An easy way to make the updated
   payloads available is to use Netlisten to serve them at a local server for
   Trident to pull from.

   ```bash
   cp input/trident.yaml input/trident-update.yaml

   # Use an IDE or vim to update the URLs inside the Host Configuration to
   # point to the updated images.
   # E.g. http://<VM_IP_address>:<any_port_number>/files/v2/esp.rawzst

   # Build Netlisten
   make bin/netlisten

   # Run Netlisten to serve the images at the chose port number
   bin/netlisten -s artifacts/test-image -p <any_port_number>
   ```

8. Inside the VM, request an A/B update.

   ```bash
   vim trident-update.yaml

   # Copy the updated HC from input/trident-update.yaml here

   # Re-run Trident
   sudo /usr/bin/trident run -v trace -c trident-update.yaml
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
