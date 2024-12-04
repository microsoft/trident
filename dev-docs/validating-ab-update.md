# Manual A/B update validation steps

The purpose of this document is to provide a manual validation procedure for
running the A/B update flow with Trident.

## Validation steps

1. The runtime OS image payload needs to be made available for Trident to
   operate on as a local file. For example, the OS image can be bundled with
   the installer OS and referenced from the initial host configuration as
   follows:

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
            url: file:///trident_cdrom/data/esp.rawzst
            sha256: ignored
            format: raw-zst
         mountPoint:
            path: /boot/efi
            options: umask=0077
      - deviceId: root
         type: ext4
         source:
            type: image
            url: file:///trident_cdrom/data/root.rawzst
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
      - name: testing-user
         sshPublicKeys: []
         sshMode: key-only
   ```

2. In the sample host configuration above, Trident is requested to create
   **two copies of the root** partition, i.e., a volume pair with id root that
   contains two partitions root-a and root-b, and to place an image in the
   raw-zst format onto root. For feature testing, **storage.images** and
   **storage.abUpdate** sections should be modified to contain RAID arrays,
   encrypted volumes, etc., to ensure that the A/B upgrade flow succeeds when
   these special block devices are present.

3. Boot the VM with the Provisioning OS using standard `make run-netlaunch`. Do
   not use any password authorization in the HC, as the non-dev build of
   Trident would fail with that HC.

4. When the installation of the initial runtime OS is completed, log into the
   VM using SSH.

5. Download images for upgrading, e.g.:
   Note: Trident supports images with grub-noprefix rpm, Pls use the latest images for upgrading.

   ```bash
   curl -L https://hermesstorageacc.blob.core.windows.net/hermes-container/555555/esp.rawzst -o esp_v2.raw.zst
   curl -L https://hermesstorageacc.blob.core.windows.net/hermes-container/555555/root.rawzst -o root_v2.raw.zst
   ```

6. Request an A/B update by applying an edited Host Configuration. In the config
   file, update **storage.images** section to include the local URL links to the
   update images:

   ```bash
   cat > /etc/trident/config.yaml << EOF
   <body of the updated Trident HostConfig>
   EOF
   ```

7. After updating the Host Configuration, apply it by restarting Trident and
   view the Trident logs to follow the A/B update flow live:

   ```bash
   sudo trident run -v trace -c /path/to/host-config.yaml --allowed-operations stage,finalize
   ```

8. Confirm that the VM simulating a BM host reboots into the new runtime OS
image. Ssh back into the host and view the changes to the system by fetching
the host's status with `trident get`. Use commands such as `blkid` and `mount`
to confirm that the volume pairs have been correctly updated and that the
correct block devices have been mounted at the designated mountpoints.
