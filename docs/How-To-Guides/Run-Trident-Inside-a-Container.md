
# Run Trident Inside a Container

This guide explains how to run Trident inside a container.

Note: this guide will use `docker` for all code snippets.

## Steps

1. Build the Trident container image using `make
   artifacts/test-image/trident-container.tar.gz`. This Make target will build
   the Trident RPMs (`make bin/trident-rpms/azl3.tar.gz`) and then use
   [Dockerfile.runtime](../Dockerfile.runtime) to build the container image with
   all the necessary dependencies. You can find a compressed form of
   containerized Trident at `artifacts/test-image/trident-container.tar.gz`.

2. Load the Trident container image. `docker load --input
   trident-container.tar.gz`. Depending on where you choose to place the Trident
   container image, change the file path in the provided code sample.

3. Run Trident:

   ```bash
   docker run --name trident_container \
              --pull=never \
              --rm \
              --privileged \
              -v /etc/trident:/etc/trident \
              -v /etc/pki:/etc/pki:ro \
              -v /var/lib/trident:/var/lib/trident \
              -v /var/log:/var/log \
              -v /:/host \
              -v /dev:/dev \
              -v /run:/run \
              -v /sys:/sys \
              --pid host \
              --ipc host \
              trident/trident:latest [TRIDENT VERB] /etc/trident/hostconf.yaml --verbosity TRACE
   ```

   Note: By default, the Trident Host Configuration should be placed inside
   `/etc/trident/`. However, if you have placed your Host Configuration outside
   of `/etc/trident/`, please replace `/etc/trident/hostconf.yaml` with the path
   to your Host Configuration file.

   Replace `[TRIDENT VERB]` with the desired verb. For a complete explanation of
   the Trident CLI, please see the [Reference
   guide](../Reference/Trident-CLI.md).

## Explanation of Docker Command

Trident must be run in `--privileged` mode so that it has access to devices on
the host, and allows Trident to perform operations such as partitioning disks
and creating filesystems. `--pid host` and `--ipc host` allow Trident to share
the host's PID namespace and IPC resources, necessary for communicating with
other system-level tools.

### Mounted Volumes

`/etc/trident`: This is required so that Trident has access to the Host
Configuration. If the Host Configuration is not located in this directory, this
option is not required.

`/etc/pki`: This is required for Trident to be able to authenticate container
registries, in which COSI files may be stored. If the COSI file is stored or
hosted locally, it is not required to mount this certificate volume.

`/var/lib/trident`: This is the default location of the Trident datastore and
must be accessible to Trident.

`/var/log`: Trident logs and metrics are stored at `/var/log/trident-full.log`
and `/var/log/trident-metrics.jsonl`.

`/`: This is required for all Trident operations.

`/dev`: This is required for Trident's access to devices.

`/run`: Trident makes use of various systemd services which require access to
`/run`.

`/sys`: Trident makes use of various systemd services which require access to
`/sys`.