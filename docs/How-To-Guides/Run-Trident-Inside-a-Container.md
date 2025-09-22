
# Run Trident Inside a Container

This guide shows you how to run Trident inside a container for clean install and
A/B Update.

## Steps for Clean Install

1. Build the Trident container image using `make
   artifacts/test-image/trident-container.tar.gz`. This Make target will build
   the Trident RPMs (`make bin/trident-rpms/azl3.tar.gz`) and then use
   [Dockerfile.runtime](../Dockerfile.runtime) to build the container image with
   all the necessary dependencies. You can find a compressed form of
   containerized Trident at `artifacts/test-image/trident-container.tar.gz`.

2. Build an installer ISO. Please reference this (Tutorial on Building a
   Provisioning ISO)[../Tutorials/Building-a-Provisioning-ISO.md] for steps on
   how to use Prism to build an installer ISO. This is the ISO from which the
   provisioning/management OS will run. Ensure that the ISO has access to the
   Trident container image. The can be done with the following addition to your
   Prism configuration file:

    ```yaml
    additionalFiles:
      - source: # Fill in with the location of your Trident container image
        destination: /var/lib/trident/trident-container.tar.gz
    ```

   In addition, ensure that the installer ISO contains a Systemd unit file which
   will start Trident in the installer ISO on boot. An example unit file is the
   following:

    ```systemd
    [Unit]
    Description=Trident Agent
    Requires=docker.service
    After=network.target network-online.target systemd-udev-settle.service docker.service

    [Service]
    Type=oneshot
    ExecStartPre=/bin/bash -c "set -e; mkdir -p /var/lib/trident"
    ExecStartPre=/bin/bash -c "set -e; if ! docker image ls | grep -q 'trident/trident'; then docker load --input /var/lib/trident/trident-container.tar.gz; fi"
    ExecStart=docker run --name trident_container --pull=never --rm --privileged -v /etc/trident:/etc/trident -v /etc/pki:/etc/pki:ro -v /run/initramfs/live:/trident_cdrom -v /var/lib/trident:/var/lib/trident -v /var/log:/var/log -v /:/host -v /dev:/dev -v /run:/run -v /sys:/sys --pid host --ipc host trident/trident:latest install --verbosity TRACE
    StandardOutput=journal+console
    StandardError=journal+console

    [Install]
    WantedBy=multi-user.target
    ```

    This unit file first ensures that the `/var/lib/trident` directory exists on
    the ISO. Next, it checks if the `trident/trident` container image is loaded,
    and if not the image is loaded from
    `/var/lib/trident/trident-container.tar.gz`. Lastly, the service runs
    Trident in privileged mode.

3. Build a runtime OS image, i.e. a COSI file. Please reference this (Tutorial
   on Building a Deployable Image)[../Tutorials/Building-a-Deployable-Image.md].

4. If your runtime OS image does not contain the Trident container image in it,
   it is necessary to copy over the `trident-container.tar.gz` file from the
   Provisioning OS to the Runtime OS via `additionalFiles`. Make sure to update
   the `os` section of your Trident Host Configuration as follows:

   ```yaml
   os:
      additionalFiles:
         - source: "/var/lib/trident/trident-container.tar.gz"
           destination: "/var/lib/trident/trident-container.tar.gz"
   ```

   This step can be skipped if your testing host OS image already contains the
   Trident container image.

5. You can now deploy Trident using this installer ISO as well as the runtime
   image. See (How To Perform a Clean Install)[./Perform-a-Clean-Install.md] for
   the remaining steps.

## Steps for A/B Update

1. Inside the provisioned OS, create a new Host Configuration. Please reference
   (How To Configure an A/B Update Ready
   Host)[./Configure-an-ABUpdate-Ready-Host.md] for how to prepare your Host
   Configuration. The recommended location for your Host Configuration is inside
   `/etc/trident/`.

2. Create a new runtime OS image. If it does not include the Trident container
   image, ensure that you copy over the container image again in your Host
   Configuration using the `additionalFiles` API.

3. Ensure that the Trident container image is in your runtime OS. This can be
   done with `docker image ls`.

4. Run Trident:

   ```docker
   docker run --name trident_container 
              --pull=never 
              --rm 
              --privileged 
              -v /etc/trident:/etc/trident 
              -v /etc/pki:/etc/pki:ro 
              -v /var/lib/trident:/var/lib/trident 
              -v /var/log:/var/log 
              -v /:/host 
              -v /dev:/dev 
              -v /run:/run 
              -v /sys:/sys 
              --pid host 
              --ipc host 
              trident/trident:latest update /etc/trident/hostconf.yaml --verbosity TRACE
   ```

   Note: If you have placed your Host Configuration outside of `/etc/trident/`,
   please replace `/etc/trident/hostconf.yaml` with the path to your Host
   Configuration file.
