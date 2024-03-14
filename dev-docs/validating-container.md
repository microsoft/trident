# Manual container validation steps

The purpose of this document is to provide a manual validation procedure for
running Trident in a container.

## Prerequisites

1. Build the Trident container image using the instructions in the
   [README](../README.md#running-from-container).

2. To deploy it to a VM, you will need to have the image available in some
   registry. You can setup local registry and use that or push to ACR. Using
   registry with anonymous pull will make your life easier, though be careful
   what you push into such registry. For example, you could (includes the
   previous step, don't forget to update your alias):

   ```bash
   ALIAS=<alias>
   make docker-build
   docker tag docker.io/trident/trident:latest acrafoimages.azurecr.io/trident:$ALIAS
   docker push acrafoimages.azurecr.io/trident:$ALIAS
   ```

   Note that you will need to still auth to the ACR to publish, and you might
   want to logout afterwards to prevent issues with virt-deploy later.

## Deploying Trident

Since we are currently booting Provisioning OS from initrd and there are limits
on the size of the initrd, it is tricky to fit the Docker runtime into the
initrd and have it boot. So instead, we could test Trident in a container from
the runtime OS. There are two options for this, one more light weight, which
only tests basics, and one more thorough, with a small workaround.

### Light-weight validation steps

1. Boot the VM with the Provisioning OS using standard `make run-netlaunch`. Do
   not use any password auth in the HC, as the non-dev build of Trident would
   fail with that HC. Ensure that you have A/B block present in your Host Configuration.
2. Log into the VM using SSH.
3. Stop Trident, install Docker (if not in your runtime OS image already):

   ```bash
   sudo systemctl stop trident
   sudo tdnf install -y moby-engine moby-cli
   sudo systemctl start docker
   ```

4. Run Trident from the container to perform `get` for basic validation (dont
   forget to update your alias):

   ```bash
   ALIAS=<alias>
   sudo docker run -it --rm --privileged -v /etc/trident:/etc/trident -v /var/lib/trident:/var/lib/trident -v /:/host --pid host acrafoimages.azurecr.io/trident:$ALIAS get
   ```

5. Download images for upgrading, e.g.:

   ```bash
   curl -L https://hermesstorageacc.blob.core.windows.net/hermes-container/240403/esp.raw.zst -o esp.rawzst
   curl -L https://hermesstorageacc.blob.core.windows.net/hermes-container/240403/root.raw.zst -o root.rawzst
   ```

6. Patch the HC to trigger update, with some extra cleanup:

   ```bash
   sudo sed -i "s#/trident_cdrom/data#`pwd`#g" /etc/trident/config.yaml
   sudo sed -i -r 's#phonehome:.+##g' /etc/trident/config.yaml
   sudo sed -i 's#selfUpgrade: true#selfUpgrade: false#g' /etc/trident/config.yaml
   ```

7. Run Trident from the container to perform A/B update:

   ```bash
   sudo docker run -it --rm --privileged -v /etc/trident:/etc/trident -v /var/lib/trident:/var/lib/trident -v /:/host -v /dev:/dev -v /run/udev:/run/udev -v /sys:/sys -v /run/systemd:/run/systemd -v `pwd`:`pwd` --pid host acrafoimages.azurecr.io/trident:$ALIAS run -v DEBUG
   ```

8. The VM should reboot into a different image. You might see a failure of
   Trident, as the HC used might not be compatible with Trident in the new
   runtime image. You can also tell the different image if you look at the
   hostname on the login prompt.

### Thorough validation steps

We will perform steps similar to above, with some extra tweaks. We will need a
virt-deploy VM with two disks and patched Trident.

1. Deploy VM with two disks, from argus-toolkit:

   ```bash
   ./virt-deploy create :::16,16
   ./virt-deploy run
   ```

2. Patch Trident to disable safety check. You will want to comment out code
   inside `provision_host()` that checks for presence of `/proc/cmdline`.

3. Rebuild and publish Trident container image, as described in the
   [prerequisites](#prerequisites).

4. Boot the VM with the Provisioning OS using standard `make run-netlaunch`. Do
   not use any password auth in the HC, as the non-dev build of Trident would
   fail with that HC.
5. Log into the VM using SSH.
6. Stop Trident, install Docker (if not in your runtime OS image already):

   ```bash
   sudo systemctl stop trident
   sudo tdnf install -y moby-engine moby-cli
   sudo systemctl start docker
   ```

7. Run Trident from the container to perform `get` for basic validation (dont
   forget to update your alias):

   ```bash
   ALIAS=<alias>
   sudo docker run -it --rm --privileged -v /etc/trident:/etc/trident -v /var/lib/trident:/var/lib/trident -v /:/host --pid host acrafoimages.azurecr.io/trident:$ALIAS get
   ```

8. Download images for upgrading, e.g.:

   ```bash
   curl -L https://hermesstorageacc.blob.core.windows.net/hermes-container/240403/esp.raw.zst -o esp.rawzst
   curl -L https://hermesstorageacc.blob.core.windows.net/hermes-container/240403/root.raw.zst -o root.rawzst
   ```

9. Patch the HC to trigger update, with some extra cleanup (notice we will be
   deployitngo a different disk from where we are booted, fix the paths if they
   are different in your setup):

   ```bash
   sudo sed -i "s#/trident_cdrom/data#`pwd`#g" /etc/trident/config.yaml
   sudo sed -i -r 's#phonehome:.+##g' /etc/trident/config.yaml
   sudo sed -i 's#selfUpgrade: true#selfUpgrade: false#g' /etc/trident/config.yaml
   sudo sed -i 's#datastore: .*##g' /etc/trident/config.yaml
   sudo sed -i 's#/dev/disk/by-path/pci-0000:00:1f.2-ata-2#/dev/disk/by-path/pci-0000:00:1f.2-ata-3#g' /etc/trident/config.yaml
   ```

10. Create an override file, so we can try to perform clean install. This
    validates a lot more steps compared to update process. Make sure this file
    is created in an environment that can be reset, as it can lead to data loss.

    ```bash
    sudo touch /override-trident-safety-check
    ```

11. Run Trident from the container to perform clean install:

    ```bash
    sudo docker run -it --rm --privileged -v /etc/trident:/etc/trident -v /var/lib/trident:/var/lib/trident -v /:/host -v /dev:/dev -v /run/udev:/run/udev -v /sys:/sys -v /run/systemd:/run/systemd -v `pwd`:`pwd` --pid host acrafoimages.azurecr.io/trident:$ALIAS run -v DEBUG
    ```

12. The VM should reboot into a different image. You might see a failure of
    Trident, as the HC used might not be compatible with Trident in the new
    runtime image. You can also tell the different image if you look at the
    hostname on the login prompt.
